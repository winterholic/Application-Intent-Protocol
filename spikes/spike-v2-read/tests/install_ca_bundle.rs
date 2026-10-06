use spike_v2_read::{connect_owned_with_url, install_ca_bundle, ConnectError};

fn test_tls_server() -> (u16, std::sync::mpsc::Receiver<bool>) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let cert = CertificateDer::from(include_bytes!("certs/server.der").to_vec());
    let key = PrivateKeyDer::try_from(include_bytes!("certs/server-key.der").to_vec()).unwrap();
    let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = listener.accept().expect("client connects to TLS server");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let mut ssl_request = [0; 8];
        stream.read_exact(&mut ssl_request).expect("PostgreSQL SSLRequest");
        if ssl_request != [0, 0, 0, 8, 4, 210, 22, 47] {
            let _ = sender.send(false);
            return;
        }
        stream.write_all(b"S").expect("accept TLS negotiation");
        let connection = rustls::ServerConnection::new(std::sync::Arc::new(config)).unwrap();
        let mut tls_stream = rustls::StreamOwned::new(connection, stream);
        let mut startup_byte = [0; 1];
        let completed = tls_stream.read_exact(&mut startup_byte).is_ok();
        let _ = sender.send(completed);
    });

    (port, receiver)
}

#[tokio::test]
async fn default_connector_uses_installed_ca_without_disabling_hostname_checks() {
    assert!(install_ca_bundle(b"not a PEM certificate").is_err());
    assert!(install_ca_bundle(b"").is_err());
    assert!(install_ca_bundle(b"-----BEGIN PRIVATE KEY-----\nAQID\n-----END PRIVATE KEY-----\n").is_err());
    assert!(install_ca_bundle(&vec![b' '; 64 * 1024 + 1]).is_err());
    let ca = include_bytes!("certs/test-ca.pem");
    install_ca_bundle(ca).expect("first valid CA bundle installs");
    install_ca_bundle(ca).expect("identical bundle installation is idempotent");
    assert!(matches!(install_ca_bundle(include_bytes!("certs/untrusted-ca.pem")), Err(ConnectError::Policy(_))));
    assert!(matches!(install_ca_bundle(b"different invalid configuration"), Err(ConnectError::Policy(_))));

    let (port, handshake) = test_tls_server();
    let url = format!("host=localhost hostaddr=127.0.0.1 port={port} sslmode=require user=postgres");
    let result = connect_owned_with_url(&url).await;

    assert!(result.is_err(), "the test server closes before PostgreSQL startup completes");
    assert!(handshake.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "the default connector must trust a configured private CA");

    let (port, handshake) = test_tls_server();
    let wrong_hostname = format!("host=wrong.invalid hostaddr=127.0.0.1 port={port} sslmode=require user=postgres");
    assert!(connect_owned_with_url(&wrong_hostname).await.is_err());
    assert!(!handshake.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "installed CA must not bypass hostname verification");
}
