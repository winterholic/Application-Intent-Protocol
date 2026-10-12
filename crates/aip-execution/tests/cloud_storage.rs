use aip_execution::{
    gcs::{GcsConfig, GcsStore},
    objects::{CompletedPart, ObjectKey, S3Config, S3Store},
};
use reqwest::{Client, Method, StatusCode};
use std::{env, fs, future::Future, time::Duration};

fn variable(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn require_live_variables(names: &[&str], values: &[Option<String>]) -> Result<Vec<String>, String> {
    if names.len() != values.len() {
        return Err("live cloud test configuration is inconsistent".into());
    }
    names.iter().zip(values).map(|(name, value)| value.clone().ok_or_else(|| format!("required live-test variable {name} is missing"))).collect()
}

async fn preserve_primary_error<T, E, C, F, Fut>(result: Result<T, E>, cleanup: F) -> Result<T, E>
where
    E: std::fmt::Display,
    C: std::fmt::Display,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), C>>,
{
    if let Err(primary_error) = &result
        && let Err(cleanup_error) = cleanup().await
    {
        eprintln!("multipart cleanup also failed: {cleanup_error}; preserving original error: {primary_error}");
    }
    result
}

#[test]
fn live_cloud_tests_reject_missing_configuration_instead_of_skipping() {
    assert!(require_live_variables(&["AIP_GCS_BUCKET", "AIP_GCS_HMAC_ACCESS_ID"], &[None, None]).is_err());
}

#[tokio::test]
async fn multipart_cleanup_failure_preserves_the_primary_error() {
    let result = preserve_primary_error::<(), &str, &str, _, _>(Err("primary failure"), || async { Err("cleanup failure") }).await;
    assert_eq!(result, Err("primary failure"));
}

fn trusted_client(ca_pem: &[u8]) -> Client {
    let certificate = reqwest::Certificate::from_pem(ca_pem).expect("test CA must be valid PEM");
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .add_root_certificate(certificate)
        .build()
        .unwrap()
}

async fn put(client: &Client, url: &str, bytes: Vec<u8>, content_type: Option<&str>) -> Result<reqwest::Response, reqwest::Error> {
    let mut request = client.request(Method::PUT, url).header("content-length", bytes.len());
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    request.body(bytes).send().await
}

#[tokio::test]
#[ignore = "run through scripts/test-minio-object-storage.sh with the official MinIO container"]
async fn minio_https_actual_object_and_multipart_contract() {
    let [endpoint, bucket, access, secret, ca_path]: [String; 5] = require_live_variables(
        &["AIP_MINIO_ENDPOINT", "AIP_MINIO_BUCKET", "AIP_MINIO_ACCESS_KEY", "AIP_MINIO_SECRET_KEY", "AIP_MINIO_CA_PEM"],
        &[
            variable("AIP_MINIO_ENDPOINT"),
            variable("AIP_MINIO_BUCKET"),
            variable("AIP_MINIO_ACCESS_KEY"),
            variable("AIP_MINIO_SECRET_KEY"),
            variable("AIP_MINIO_CA_PEM"),
        ],
    )
    .expect("live MinIO test requires endpoint, bucket, credentials, and explicit CA")
    .try_into()
    .expect("the required MinIO variable list has five entries");
    let ca = fs::read(ca_path).expect("read MinIO test CA");
    let client = trusted_client(&ca);
    let base = S3Store::new(S3Config::new(endpoint, bucket, "us-east-1", access, secret).with_root_certificate_pem(&ca)).unwrap();
    let owner = format!("minio-integration-{}", uuid::Uuid::new_v4());
    let store = base.for_owner(&owner).unwrap();

    let key = ObjectKey::generate();
    let bytes = vec![0x5a; 2 * 1024 * 1024];
    let url = store.presign_put_sized(&key, "application/octet-stream", bytes.len() as u64, 300).unwrap();
    assert_eq!(put(&client, &url, bytes.clone(), Some("application/octet-stream")).await.unwrap().status(), StatusCode::OK);

    let get = store.presign_get(&key, 300).unwrap();
    let response = client.get(get).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-disposition").unwrap(), "attachment");
    assert_eq!(response.bytes().await.unwrap().as_ref(), bytes.as_slice());
    let untrusted_client = Client::builder().https_only(true).no_proxy().build().unwrap();
    assert!(
        untrusted_client.get(store.presign_head(&key, 300).unwrap()).send().await.is_err(),
        "the test server certificate must fail without its explicit CA"
    );
    let head = store.presign_head(&key, 300).unwrap();
    assert_eq!(client.head(head).send().await.unwrap().status(), StatusCode::OK);

    let wrong_size = store.presign_put_sized(&ObjectKey::generate(), "application/octet-stream", 12, 300).unwrap();
    assert_eq!(put(&client, &wrong_size, vec![1; 13], Some("application/octet-stream")).await.unwrap().status(), StatusCode::FORBIDDEN);
    let mut tampered = store.presign_get(&key, 300).unwrap();
    tampered.push_str("&untrusted=1");
    assert_eq!(client.get(tampered).send().await.unwrap().status(), StatusCode::FORBIDDEN);

    let multipart = store.create_multipart("application/octet-stream").await.unwrap();
    let first = vec![0x11; 5 * 1024 * 1024];
    let second = vec![0x22; 257];
    let mut completed = Vec::new();
    for (number, part) in [(1u16, first), (2u16, second)] {
        let part_url = store.presign_part_sized(&multipart.key, &multipart.upload_id, number, part.len() as u64, 300).unwrap();
        let response = put(&client, &part_url, part, None).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers().get("etag").unwrap().to_str().unwrap().to_owned();
        completed.push(CompletedPart { number, etag });
    }
    store.complete_multipart(&multipart, &completed).await.unwrap();
    let multipart_data = client.get(store.presign_get(&multipart.key, 300).unwrap()).send().await.unwrap();
    assert_eq!(multipart_data.status(), StatusCode::OK);
    assert_eq!(multipart_data.bytes().await.unwrap().len(), 5 * 1024 * 1024 + 257);

    let pending = store.create_multipart("application/octet-stream").await.unwrap();
    store.abort_multipart(&pending).await.unwrap();
    assert_eq!(client.delete(store.presign_delete(&key, 300).unwrap()).send().await.unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(client.get(store.presign_get(&key, 300).unwrap()).send().await.unwrap().status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires a real GCS bucket and HMAC credentials; never exercised by the MinIO emulator"]
async fn gcs_hmac_actual_xml_api_roundtrip_when_credentials_are_configured() {
    let [bucket, access, secret]: [String; 3] = require_live_variables(
        &["AIP_GCS_BUCKET", "AIP_GCS_HMAC_ACCESS_ID", "AIP_GCS_HMAC_SECRET"],
        &[variable("AIP_GCS_BUCKET"), variable("AIP_GCS_HMAC_ACCESS_ID"), variable("AIP_GCS_HMAC_SECRET")],
    )
    .expect("live GCS test requires a real bucket and HMAC credentials")
    .try_into()
    .expect("the required GCS variable list has three entries");
    let client =
        Client::builder().https_only(true).redirect(reqwest::redirect::Policy::none()).no_proxy().timeout(Duration::from_secs(60)).build().unwrap();
    let base = GcsStore::new(GcsConfig::new(bucket, access, secret)).unwrap();
    let store = base.for_owner(&format!("live-gcs-integration-{}", uuid::Uuid::new_v4())).unwrap();
    let key = ObjectKey::generate();
    let bytes = vec![0x34; 2 * 1024 * 1024];
    let put_url = store.presign_put_sized(&key, "application/octet-stream", bytes.len() as u64, 300).unwrap();
    assert!(put(&client, &put_url, bytes.clone(), Some("application/octet-stream")).await.unwrap().status().is_success());
    assert!(client.head(store.presign_head(&key, 300).unwrap()).send().await.unwrap().status().is_success());
    let downloaded = client.get(store.presign_get(&key, 300).unwrap()).send().await.unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(downloaded.bytes().await.unwrap().as_ref(), bytes.as_slice());
    assert!(client.delete(store.presign_delete(&key, 300).unwrap()).send().await.unwrap().status().is_success());

    let multipart = store.create_multipart("application/octet-stream").await.unwrap();
    let mut multipart_completed = false;
    let multipart_result: Result<(), String> = async {
        let first = vec![0x11; 5 * 1024 * 1024];
        let final_part = vec![0x22; 193];
        let mut completed = Vec::new();
        for (number, part) in [(1u16, first), (2u16, final_part)] {
            let url =
                store.presign_part_sized(&multipart.key, &multipart.upload_id, number, part.len() as u64, 300).map_err(|error| error.to_string())?;
            let response = put(&client, &url, part, None).await.map_err(|error| error.to_string())?;
            if !response.status().is_success() {
                return Err(format!("GCS multipart part returned {}", response.status()));
            }
            let etag = response.headers().get("etag").ok_or_else(|| "GCS multipart response omitted ETag".to_owned())?;
            completed.push(CompletedPart { number, etag: etag.to_str().map_err(|error| error.to_string())?.to_owned() });
        }
        store.complete_multipart(&multipart, &completed).await.map_err(|error| error.to_string())?;
        multipart_completed = true;
        let multipart_get =
            client.get(store.presign_get(&multipart.key, 300).map_err(|error| error.to_string())?).send().await.map_err(|error| error.to_string())?;
        if multipart_get.status() != StatusCode::OK {
            return Err(format!("GCS multipart download returned {}", multipart_get.status()));
        }
        let length = multipart_get.bytes().await.map_err(|error| error.to_string())?.len();
        if length != 5 * 1024 * 1024 + 193 {
            return Err(format!("GCS multipart object had {length} bytes"));
        }
        Ok(())
    }
    .await;
    let multipart_result = preserve_primary_error(multipart_result, || async {
        if multipart_completed {
            let url = store.presign_delete(&multipart.key, 300).map_err(|error| error.to_string())?;
            let response = client.delete(url).send().await.map_err(|error| error.to_string())?;
            if !response.status().is_success() {
                return Err(format!("GCS cleanup delete returned {}", response.status()));
            }
            Ok(())
        } else {
            store.abort_multipart(&multipart).await.map_err(|error| error.to_string())
        }
    })
    .await;
    multipart_result.unwrap_or_else(|error| panic!("GCS multipart contract failed: {error}"));
    let signed_failure = store.presign_put_sized(&ObjectKey::generate(), "application/octet-stream", 9, 300).unwrap();
    assert_eq!(put(&client, &signed_failure, vec![0x44; 10], Some("application/octet-stream")).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert!(client.delete(store.presign_delete(&multipart.key, 300).unwrap()).send().await.unwrap().status().is_success());
}
