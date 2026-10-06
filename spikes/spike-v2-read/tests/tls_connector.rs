use spike_v2_read::{connect_owned_with_url, connect_owned_with_url_and_ca};

#[tokio::test]
async fn remote_disable_is_rejected_before_any_network_connection() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let accept_thread = std::thread::spawn(move || {
        for _ in 0..100 {
            match listener.accept() {
                Ok(_) => return true,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
        false
    });
    let url = format!("host=remote.invalid hostaddr=127.0.0.1 port={port} sslmode=disable user=postgres");
    let error = match connect_owned_with_url(&url).await {
        Err(error) => error,
        Ok(_) => panic!("remote plaintext connection unexpectedly succeeded"),
    };

    assert!(error.to_string().contains("remote PostgreSQL connection requires sslmode=require"), "unexpected error: {error}");
    assert!(!accept_thread.join().unwrap(), "policy rejection must precede TCP connect");
}

#[tokio::test]
async fn remote_prefer_is_rejected_before_any_network_connection() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let accept_thread = std::thread::spawn(move || {
        for _ in 0..100 {
            match listener.accept() {
                Ok(_) => return true,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
        false
    });
    let url = format!("host=remote.invalid hostaddr=127.0.0.1 port={port} sslmode=prefer user=postgres");
    let error = match connect_owned_with_url(&url).await {
        Err(error) => error,
        Ok(_) => panic!("remote plaintext fallback unexpectedly succeeded"),
    };

    assert!(error.to_string().contains("remote PostgreSQL connection requires sslmode=require"), "unexpected error: {error}");
    assert!(!accept_thread.join().unwrap(), "policy rejection must precede TCP connect");
}

#[tokio::test]
async fn loopback_require_sends_postgres_tls_negotiation() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("client connects to loopback");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
        let mut ssl_request = [0; 8];
        stream.read_exact(&mut ssl_request).expect("PostgreSQL SSLRequest");
        if ssl_request != [0, 0, 0, 8, 4, 210, 22, 47] {
            return false;
        }
        stream.write_all(b"S").expect("accept TLS negotiation");
        let mut tls_record = [0; 1];
        stream.read_exact(&mut tls_record).expect("TLS ClientHello");
        tls_record[0] == 0x16
    });

    let url = format!("host=127.0.0.1 port={port} sslmode=require user=postgres");
    let result = connect_owned_with_url(&url).await;
    assert!(result.is_err(), "test server closes after observing the TLS ClientHello");
    assert!(server_thread.join().unwrap(), "sslmode=require must negotiate TLS before startup");
}

fn test_tls_server() -> (u16, std::sync::mpsc::Receiver<bool>) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let cert = CertificateDer::from(include_bytes!("certs/server.der").to_vec());
    let key = PrivateKeyDer::try_from(include_bytes!("certs/server-key.der").to_vec()).unwrap();
    let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        use std::io::Read;
        use std::io::Write;
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
        let trusted_tls_handshake = match tls_stream.read_exact(&mut startup_byte) {
            Ok(()) => true,
            Err(_) => false,
        };
        let _ = sender.send(trusted_tls_handshake);
    });

    (port, receiver)
}

#[tokio::test]
async fn custom_ca_and_matching_hostname_complete_tls_handshake() {
    let (port, handshake) = test_tls_server();
    let url = format!("host=localhost hostaddr=127.0.0.1 port={port} sslmode=require user=postgres");
    let ca = include_bytes!("certs/test-ca.pem");
    let result = connect_owned_with_url_and_ca(&url, ca).await;
    let result_summary = match &result {
        Ok(_) => "unexpected database connection success".into(),
        Err(error) => error.to_string(),
    };

    assert!(result.is_err(), "the test server closes before PostgreSQL startup completes");
    assert!(
        handshake.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
        "trusted CA and matching hostname must pass TLS verification: {result_summary}"
    );
}

#[tokio::test]
async fn trusted_ca_does_not_allow_a_mismatched_server_hostname() {
    let (port, handshake) = test_tls_server();
    let url = format!("host=wrong.invalid hostaddr=127.0.0.1 port={port} sslmode=require user=postgres");
    let ca = include_bytes!("certs/test-ca.pem");
    let result = connect_owned_with_url_and_ca(&url, ca).await;

    assert!(result.is_err());
    assert!(!handshake.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "certificate SAN for localhost must not authenticate wrong.invalid");
}

#[tokio::test]
async fn an_untrusted_ca_does_not_complete_the_tls_handshake() {
    let (port, handshake) = test_tls_server();
    let url = format!("host=localhost hostaddr=127.0.0.1 port={port} sslmode=require user=postgres");
    let ca = include_bytes!("certs/untrusted-ca.pem");
    let result = connect_owned_with_url_and_ca(&url, ca).await;

    assert!(result.is_err());
    assert!(
        !handshake.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
        "a certificate signed by an untrusted CA must fail TLS verification"
    );
}
