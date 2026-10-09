use std::net::SocketAddr;
use std::sync::Arc;

use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::protocol::PeerMessage;

/// Server name used in every peer handshake. Peers are identified by their
/// certificate fingerprint, not by name.
const PEER_SERVER_NAME: &str = "artemis-peer";

/// QUIC transport layer for peer-to-peer connections.
///
/// Every peer has a fresh self-signed certificate. Peers exchange its SHA-256
/// fingerprint through signaling, and a connection is trusted only when the
/// other side proves it holds the key of the certificate with that
/// fingerprint: the dialer pins the fingerprint in the handshake, and the
/// listener requires a client certificate and exposes its fingerprint
/// ([`PeerConnection::peer_fingerprint`]) for the caller to compare.
pub struct QuicTransport {
    endpoint: Endpoint,
    cert_fingerprint: String,
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl QuicTransport {
    /// Create a new QUIC transport bound to the given address.
    /// Generates a self-signed certificate for TLS.
    pub fn bind(bind_addr: SocketAddr) -> Result<Self, TransportError> {
        let certified_key = rcgen::generate_simple_self_signed(vec![PEER_SERVER_NAME.to_string()])
            .map_err(|e| TransportError::CertGeneration(e.to_string()))?;

        let cert = CertificateDer::from(certified_key.cert.der().to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified_key.signing_key.serialize_der(),
        ));
        let cert_fingerprint = hex_fingerprint(&cert);
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        // Server side: a client certificate is mandatory and must sign the
        // handshake; which certificate is acceptable is the caller's check.
        let server_crypto = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| TransportError::TlsConfig(e.to_string()))?
            .with_client_cert_verifier(Arc::new(PeerCertVerifier::any(&provider)))
            .with_single_cert(vec![cert.clone()], key.clone_key())
            .map_err(|e| TransportError::TlsConfig(e.to_string()))?;

        let server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| TransportError::TlsConfig(e.to_string()))?,
        ));

        // No default client config: every outgoing connection pins the
        // peer's fingerprint (see `connect_to_peer`).
        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| TransportError::Bind(e.to_string()))?;

        info!("QUIC transport bound to {}", bind_addr);
        Ok(Self {
            endpoint,
            cert_fingerprint,
            cert,
            key,
            provider,
        })
    }

    /// Get the local address this transport is bound to.
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.endpoint
            .local_addr()
            .map_err(|e| TransportError::Bind(e.to_string()))
    }

    /// Get the certificate fingerprint (for sharing via signaling).
    pub fn cert_fingerprint(&self) -> &str {
        &self.cert_fingerprint
    }

    /// Connect to a peer at the given address. The handshake fails unless the
    /// peer presents the certificate whose fingerprint it published through
    /// signaling (`expected_fingerprint`) and proves it holds its key.
    pub async fn connect_to_peer(
        &self,
        addr: SocketAddr,
        expected_fingerprint: &str,
    ) -> Result<PeerConnection, TransportError> {
        let verifier = PeerCertVerifier::pinned(&self.provider, expected_fingerprint);
        let client_crypto = rustls::ClientConfig::builder_with_provider(self.provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| TransportError::TlsConfig(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_client_auth_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(|e| TransportError::TlsConfig(e.to_string()))?;
        let client_config = ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
                .map_err(|e| TransportError::TlsConfig(e.to_string()))?,
        ));

        let connection = self
            .endpoint
            .connect_with(client_config, addr, PEER_SERVER_NAME)
            .map_err(|e| TransportError::Connect(e.to_string()))?
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;

        info!("Connected to peer at {}", addr);
        Ok(PeerConnection { connection })
    }

    /// Accept an incoming peer connection.
    pub async fn accept_peer(&self) -> Result<PeerConnection, TransportError> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| TransportError::Accept("endpoint closed".to_string()))?;

        let connection = incoming
            .await
            .map_err(|e| TransportError::Accept(e.to_string()))?;

        info!(
            "Accepted peer connection from {}",
            connection.remote_address()
        );
        Ok(PeerConnection { connection })
    }

    /// Run the accept loop, sending new peer connections to the channel.
    pub async fn accept_loop(&self, tx: mpsc::UnboundedSender<PeerConnection>) {
        loop {
            match self.accept_peer().await {
                Ok(conn) => {
                    if tx.send(conn).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("Failed to accept peer: {}", e);
                }
            }
        }
    }

    /// Close the transport.
    pub fn close(&self) {
        self.endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    }
}

/// A connection to a single peer.
pub struct PeerConnection {
    connection: Connection,
}

impl PeerConnection {
    /// SHA-256 fingerprint of the certificate the peer proved it holds, in the
    /// format of [`QuicTransport::cert_fingerprint`].
    ///
    /// On an accepted connection, compare it with the fingerprint from the
    /// peer's signaling request before trusting anything it sends.
    pub fn peer_fingerprint(&self) -> Option<String> {
        let certs = self
            .connection
            .peer_identity()?
            .downcast::<Vec<CertificateDer<'static>>>()
            .ok()?;
        certs.first().map(hex_fingerprint)
    }

    /// Get the remote address of this peer.
    pub fn remote_addr(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// Open a bidirectional stream for sending/receiving messages.
    pub async fn open_stream(&self) -> Result<(PeerSender, PeerReceiver), TransportError> {
        let (send, recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|e| TransportError::Stream(e.to_string()))?;

        Ok((PeerSender { send }, PeerReceiver { recv }))
    }

    /// Accept a bidirectional stream from the peer.
    pub async fn accept_stream(&self) -> Result<(PeerSender, PeerReceiver), TransportError> {
        let (send, recv) = self
            .connection
            .accept_bi()
            .await
            .map_err(|e| TransportError::Stream(e.to_string()))?;

        Ok((PeerSender { send }, PeerReceiver { recv }))
    }

    /// Run a message loop: send outgoing messages, receive incoming ones.
    /// Returns channels for the application to use.
    pub async fn run_message_loop(
        self,
    ) -> Result<
        (
            mpsc::UnboundedSender<PeerMessage>,
            mpsc::UnboundedReceiver<PeerMessage>,
        ),
        TransportError,
    > {
        let (app_tx, mut app_rx) = mpsc::unbounded_channel::<PeerMessage>();
        let (peer_tx, peer_rx) = mpsc::unbounded_channel::<PeerMessage>();

        // Open a stream for sending
        let (mut sender, _) = self.open_stream().await?;

        // Spawn sender task
        tokio::spawn(async move {
            while let Some(msg) = app_rx.recv().await {
                if sender.send_message(&msg).await.is_err() {
                    break;
                }
            }
        });

        // Spawn receiver task — accepts streams and forwards messages
        let connection = self.connection.clone();
        tokio::spawn(async move {
            loop {
                match connection.accept_bi().await {
                    Ok((_, recv)) => {
                        let tx = peer_tx.clone();
                        tokio::spawn(async move {
                            let mut receiver = PeerReceiver { recv };
                            while let Ok(Some(msg)) = receiver.recv_message().await {
                                if tx.send(msg).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("Peer connection closed: {}", e);
                        break;
                    }
                }
            }
        });

        Ok((app_tx, peer_rx))
    }

    /// Close this connection.
    pub fn close(&self) {
        self.connection.close(quinn::VarInt::from_u32(0), b"bye");
    }
}

/// Sender side of a peer stream.
pub struct PeerSender {
    send: SendStream,
}

impl PeerSender {
    /// Send a PeerMessage (length-prefixed JSON).
    pub async fn send_message(&mut self, msg: &PeerMessage) -> Result<(), TransportError> {
        let json = serde_json::to_vec(msg).map_err(|e| TransportError::Serialize(e.to_string()))?;
        let len = (json.len() as u32).to_be_bytes();
        self.send
            .write_all(&len)
            .await
            .map_err(|e| TransportError::Stream(e.to_string()))?;
        self.send
            .write_all(&json)
            .await
            .map_err(|e| TransportError::Stream(e.to_string()))?;
        Ok(())
    }
}

/// Receiver side of a peer stream.
pub struct PeerReceiver {
    recv: RecvStream,
}

impl PeerReceiver {
    /// Receive a PeerMessage (length-prefixed JSON).
    pub async fn recv_message(&mut self) -> Result<Option<PeerMessage>, TransportError> {
        let mut len_buf = [0u8; 4];
        match self.recv.read_exact(&mut len_buf).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(_)) => return Ok(None),
            Err(e) => return Err(TransportError::Stream(e.to_string())),
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 16 * 1024 * 1024 {
            return Err(TransportError::MessageTooLarge(len));
        }
        let mut buf = vec![0u8; len];
        self.recv
            .read_exact(&mut buf)
            .await
            .map_err(|e| TransportError::Stream(e.to_string()))?;
        let msg =
            serde_json::from_slice(&buf).map_err(|e| TransportError::Deserialize(e.to_string()))?;
        Ok(Some(msg))
    }
}

/// Hex SHA-256 fingerprint of a certificate, as `AB:CD:...` (32 bytes).
fn hex_fingerprint(cert: &CertificateDer<'_>) -> String {
    Sha256::digest(cert)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Certificate verifier for self-signed peer certificates.
///
/// There is no certificate authority: a peer is identified by the SHA-256
/// fingerprint of its certificate. The handshake signature is always verified
/// against the presented certificate, so the peer proves it holds the key.
#[derive(Debug)]
struct PeerCertVerifier {
    /// Fingerprint the certificate must have, or `None` to accept any
    /// certificate and leave the check to the caller (listener side).
    expected: Option<String>,
    provider: Arc<CryptoProvider>,
}

impl PeerCertVerifier {
    fn pinned(provider: &Arc<CryptoProvider>, fingerprint: &str) -> Self {
        Self {
            expected: Some(fingerprint.to_ascii_uppercase()),
            provider: provider.clone(),
        }
    }

    fn any(provider: &Arc<CryptoProvider>) -> Self {
        Self {
            expected: None,
            provider: provider.clone(),
        }
    }

    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        match &self.expected {
            Some(expected) if *expected != hex_fingerprint(cert) => {
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ))
            }
            _ => Ok(()),
        }
    }

    fn tls12(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, cert, dss, algorithms)
    }

    fn tls13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, cert, dss, algorithms)
    }

    fn schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ServerCertVerifier for PeerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl ClientCertVerifier for PeerCertVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("certificate generation failed: {0}")]
    CertGeneration(String),
    #[error("TLS configuration error: {0}")]
    TlsConfig(String),
    #[error("failed to bind: {0}")]
    Bind(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("accept failed: {0}")]
    Accept(String),
    #[error("stream error: {0}")]
    Stream(String),
    #[error("serialization error: {0}")]
    Serialize(String),
    #[error("deserialization error: {0}")]
    Deserialize(String),
    #[error("message too large: {0} bytes")]
    MessageTooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn localhost() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    #[tokio::test]
    async fn pinned_fingerprint_connects_and_both_sides_see_the_other() {
        let alice = QuicTransport::bind(localhost()).unwrap();
        let bob = QuicTransport::bind(localhost()).unwrap();
        let bob_addr = bob.local_addr().unwrap();

        let (dialed, accepted) = tokio::join!(
            alice.connect_to_peer(bob_addr, bob.cert_fingerprint()),
            bob.accept_peer()
        );
        let (dialed, accepted) = (dialed.unwrap(), accepted.unwrap());

        assert_eq!(
            dialed.peer_fingerprint().as_deref(),
            Some(bob.cert_fingerprint())
        );
        assert_eq!(
            accepted.peer_fingerprint().as_deref(),
            Some(alice.cert_fingerprint())
        );
    }

    #[tokio::test]
    async fn wrong_fingerprint_is_refused() {
        let alice = QuicTransport::bind(localhost()).unwrap();
        let bob = QuicTransport::bind(localhost()).unwrap();
        let mallory = QuicTransport::bind(localhost()).unwrap();
        let bob_addr = bob.local_addr().unwrap();

        // Alice expects Mallory's certificate but reaches Bob.
        let accept = tokio::spawn(async move { bob.accept_peer().await.is_ok() });
        let dialed = alice
            .connect_to_peer(bob_addr, mallory.cert_fingerprint())
            .await;
        assert!(dialed.is_err());
        alice.close();
        let _ = accept.await;
    }

    #[test]
    fn fingerprint_is_sha256() {
        let cert = CertificateDer::from(b"abc".to_vec());
        assert_eq!(
            hex_fingerprint(&cert).replace(':', ""),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
}
