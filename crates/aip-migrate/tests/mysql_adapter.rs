#[tokio::test]
async fn mysql_schemes_fail_closed_with_adapter_specific_error() {
    for url in ["mysql://user:secret@127.0.0.1:3306/facereview", "mariadb://user:secret@127.0.0.1:3306/facereview"] {
        let error = aip_migrate::external::introspect(url, "facereview").await.unwrap_err();
        assert_eq!(error["code"], "DB_ADAPTER_UNSUPPORTED");
        assert!(!error.to_string().contains("secret"));
    }
}
