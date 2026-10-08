use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tracing::{info, warn};

/// Public STUN servers for NAT traversal.
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
];

/// STUN Binding Request magic cookie and header.
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;
const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_RESPONSE: u16 = 0x0101;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Discover our public IP:port using STUN.
pub async fn discover_public_endpoint() -> Result<SocketAddr, StunError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| StunError::Bind(e.to_string()))?;

    for server in STUN_SERVERS {
        match query_stun_server(&socket, server).await {
            Ok(addr) => {
                info!("STUN discovered public endpoint: {}", addr);
                return Ok(addr);
            }
            Err(e) => {
                warn!("STUN query to {} failed: {}", server, e);
            }
        }
    }

    Err(StunError::AllServersFailed)
}

async fn query_stun_server(socket: &UdpSocket, server: &str) -> Result<SocketAddr, StunError> {
    let server_addr: SocketAddr = tokio::net::lookup_host(server)
        .await
        .map_err(|e| StunError::Dns(e.to_string()))?
        .next()
        .ok_or_else(|| StunError::Dns("no addresses resolved".to_string()))?;

    // Build STUN Binding Request
    let transaction_id: [u8; 12] = rand::random();
    let mut request = Vec::with_capacity(20);
    request.extend_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    request.extend_from_slice(&0u16.to_be_bytes()); // message length (no attributes)
    request.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    request.extend_from_slice(&transaction_id);

    socket
        .send_to(&request, server_addr)
        .await
        .map_err(|e| StunError::Network(e.to_string()))?;

    // Wait for response with timeout
    let mut buf = [0u8; 512];
    let len = tokio::time::timeout(std::time::Duration::from_secs(3), socket.recv(&mut buf))
        .await
        .map_err(|_| StunError::Timeout)?
        .map_err(|e| StunError::Network(e.to_string()))?;

    parse_stun_response(&buf[..len], &transaction_id)
}

fn parse_stun_response(data: &[u8], expected_txn: &[u8; 12]) -> Result<SocketAddr, StunError> {
    if data.len() < 20 {
        return Err(StunError::InvalidResponse("too short".to_string()));
    }

    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let magic = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

    if msg_type != STUN_BINDING_RESPONSE {
        return Err(StunError::InvalidResponse(format!(
            "unexpected message type: 0x{:04X}",
            msg_type
        )));
    }

    if magic != STUN_MAGIC_COOKIE {
        return Err(StunError::InvalidResponse("bad magic cookie".to_string()));
    }

    // Verify transaction ID
    if &data[8..20] != expected_txn {
        return Err(StunError::InvalidResponse(
            "transaction ID mismatch".to_string(),
        ));
    }

    // Parse attributes
    let attrs = &data[20..20 + msg_len.min(data.len() - 20)];
    let mut offset = 0;

    while offset + 4 <= attrs.len() {
        let attr_type = u16::from_be_bytes([attrs[offset], attrs[offset + 1]]);
        let attr_len = u16::from_be_bytes([attrs[offset + 2], attrs[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > attrs.len() {
            break;
        }

        let attr_data = &attrs[offset..offset + attr_len];

        match attr_type {
            STUN_ATTR_XOR_MAPPED_ADDRESS => {
                return parse_xor_mapped_address(attr_data);
            }
            STUN_ATTR_MAPPED_ADDRESS => {
                return parse_mapped_address(attr_data);
            }
            _ => {}
        }

        // Attributes are padded to 4-byte boundary
        offset += (attr_len + 3) & !3;
    }

    Err(StunError::InvalidResponse(
        "no mapped address attribute found".to_string(),
    ))
}

fn parse_xor_mapped_address(data: &[u8]) -> Result<SocketAddr, StunError> {
    if data.len() < 8 {
        return Err(StunError::InvalidResponse(
            "XOR-MAPPED-ADDRESS too short".to_string(),
        ));
    }

    let family = data[1];
    let xor_port = u16::from_be_bytes([data[2], data[3]]) ^ (STUN_MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            // IPv4
            let xor_ip =
                u32::from_be_bytes([data[4], data[5], data[6], data[7]]) ^ STUN_MAGIC_COOKIE;
            let ip = std::net::Ipv4Addr::from(xor_ip);
            Ok(SocketAddr::new(std::net::IpAddr::V4(ip), xor_port))
        }
        0x02 => {
            // IPv6 — for now just support IPv4
            Err(StunError::InvalidResponse(
                "IPv6 not yet supported".to_string(),
            ))
        }
        _ => Err(StunError::InvalidResponse(format!(
            "unknown address family: {}",
            family
        ))),
    }
}

fn parse_mapped_address(data: &[u8]) -> Result<SocketAddr, StunError> {
    if data.len() < 8 {
        return Err(StunError::InvalidResponse(
            "MAPPED-ADDRESS too short".to_string(),
        ));
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Ok(SocketAddr::new(std::net::IpAddr::V4(ip), port))
        }
        _ => Err(StunError::InvalidResponse(format!(
            "unknown address family: {}",
            family
        ))),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StunError {
    #[error("failed to bind UDP socket: {0}")]
    Bind(String),
    #[error("DNS resolution failed: {0}")]
    Dns(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("STUN request timed out")]
    Timeout,
    #[error("invalid STUN response: {0}")]
    InvalidResponse(String),
    #[error("all STUN servers failed")]
    AllServersFailed,
}
