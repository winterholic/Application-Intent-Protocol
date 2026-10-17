//! Lightweight, pure-Rust MySQL 8.4 wire protocol framing and packet parser.
//!
//! Developed to advance MySQL driver connectivity within AIP without external dependencies
//! when external crate downloads are unavailable in restricted sandbox environments.
use std::fmt;

/// Standard MySQL wire protocol packet header (4 bytes).
/// 3 bytes: payload length (little-endian), 1 byte: sequence ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub payload_length: u32,
    pub sequence_id: u8,
}

impl PacketHeader {
    pub const HEADER_SIZE: usize = 4;

    pub fn encode(&self) -> [u8; 4] {
        [
            (self.payload_length & 0xFF) as u8,
            ((self.payload_length >> 8) & 0xFF) as u8,
            ((self.payload_length >> 16) & 0xFF) as u8,
            self.sequence_id,
        ]
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::HEADER_SIZE {
            return None;
        }
        let len = (bytes[0] as u32) | ((bytes[1] as u32) << 8) | ((bytes[2] as u32) << 16);
        let seq = bytes[3];
        Some(Self {
            payload_length: len,
            sequence_id: seq,
        })
    }
}

/// Raw MySQL packet containing sequence id and payload bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RawPacket {
    pub sequence_id: u8,
    pub payload: Vec<u8>,
}

impl fmt::Debug for RawPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPacket")
            .field("sequence_id", &self.sequence_id)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl RawPacket {
    pub fn new(sequence_id: u8, payload: Vec<u8>) -> Self {
        Self { sequence_id, payload }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let header = PacketHeader {
            payload_length: self.payload.len() as u32,
            sequence_id: self.sequence_id,
        };
        let mut buf = Vec::with_capacity(PacketHeader::HEADER_SIZE + self.payload.len());
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn parse(buf: &[u8]) -> Result<Option<(Self, usize)>, WireError> {
        if buf.len() < PacketHeader::HEADER_SIZE {
            return Ok(None);
        }
        let header = PacketHeader::decode(buf).ok_or(WireError::InvalidPacketHeader)?;
        let total_size = PacketHeader::HEADER_SIZE + header.payload_length as usize;
        if buf.len() < total_size {
            return Ok(None);
        }
        let payload = buf[PacketHeader::HEADER_SIZE..total_size].to_vec();
        Ok(Some((Self {
            sequence_id: header.sequence_id,
            payload,
        }, total_size)))
    }
}

/// Errors occurring during MySQL wire protocol operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    InvalidPacketHeader,
    UnexpectedPacketType(u8),
    ProtocolMismatch(String),
    ServerError { code: u16, sql_state: String, message: String },
    BufferTooShort,
    IoError(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPacketHeader => write!(f, "invalid MySQL packet header"),
            Self::UnexpectedPacketType(b) => write!(f, "unexpected MySQL packet header indicator 0x{b:02x}"),
            Self::ProtocolMismatch(s) => write!(f, "protocol mismatch: {s}"),
            Self::ServerError { code, sql_state, message } => write!(f, "MySQL server error [{code}] ({sql_state}): {message}"),
            Self::BufferTooShort => write!(f, "wire buffer too short for expected packet data"),
            Self::IoError(s) => write!(f, "wire I/O error: {s}"),
        }
    }
}

impl std::error::Error for WireError {}

/// Server capabilities flags defined in MySQL 8.4 reference manual.
pub mod capabilities {
    pub const CLIENT_LONG_PASSWORD: u32 = 1;
    pub const CLIENT_FOUND_ROWS: u32 = 2;
    pub const CLIENT_LONG_FLAG: u32 = 4;
    pub const CLIENT_CONNECT_WITH_DB: u32 = 8;
    pub const CLIENT_NO_SCHEMA: u32 = 16;
    pub const CLIENT_COMPRESS: u32 = 32;
    pub const CLIENT_ODBC: u32 = 64;
    pub const CLIENT_LOCAL_FILES: u32 = 128;
    pub const CLIENT_IGNORE_SPACE: u32 = 256;
    pub const CLIENT_PROTOCOL_41: u32 = 512;
    pub const CLIENT_INTERACTIVE: u32 = 1024;
    pub const CLIENT_SSL: u32 = 2048;
    pub const CLIENT_IGNORE_SIGPIPE: u32 = 4096;
    pub const CLIENT_TRANSACTIONS: u32 = 8192;
    pub const CLIENT_RESERVED: u32 = 16384;
    pub const CLIENT_SECURE_CONNECTION: u32 = 32768;
    pub const CLIENT_MULTI_STATEMENTS: u32 = 1 << 16;
    pub const CLIENT_MULTI_RESULTS: u32 = 1 << 17;
    pub const CLIENT_PS_MULTI_RESULTS: u32 = 1 << 18;
    pub const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
    pub const CLIENT_CONNECT_ATTRS: u32 = 1 << 20;
    pub const CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA: u32 = 1 << 21;
}

/// Parsed Initial Handshake Packet (Protocol::HandshakeV10) from MySQL 8.4 server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeV10 {
    pub protocol_version: u8,
    pub server_version: String,
    pub connection_id: u32,
    pub auth_plugin_data: Vec<u8>,
    pub capability_flags: u32,
    pub character_set: u8,
    pub status_flags: u16,
    pub auth_plugin_name: Option<String>,
}

impl HandshakeV10 {
    pub fn parse(payload: &[u8]) -> Result<Self, WireError> {
        if payload.is_empty() {
            return Err(WireError::BufferTooShort);
        }
        let protocol_version = payload[0];
        if protocol_version != 10 {
            return Err(WireError::ProtocolMismatch(format!("expected handshake v10, found {protocol_version}")));
        }

        // null-terminated server version
        let mut idx = 1;
        let s_end = payload[idx..].iter().position(|&b| b == 0).ok_or(WireError::BufferTooShort)?;
        let server_version = String::from_utf8_lossy(&payload[idx..idx + s_end]).into_owned();
        idx += s_end + 1;

        if payload.len() < idx + 4 + 8 + 1 + 2 {
            return Err(WireError::BufferTooShort);
        }

        let connection_id = u32::from_le_bytes(payload[idx..idx + 4].try_into().unwrap());
        idx += 4;

        let mut auth_data = payload[idx..idx + 8].to_vec();
        idx += 8;

        // 1 byte filter (0x00)
        idx += 1;

        let cap_lower = u16::from_le_bytes(payload[idx..idx + 2].try_into().unwrap()) as u32;
        idx += 2;

        let character_set = payload[idx];
        idx += 1;

        let status_flags = u16::from_le_bytes(payload[idx..idx + 2].try_into().unwrap());
        idx += 2;

        let cap_upper = u16::from_le_bytes(payload[idx..idx + 2].try_into().unwrap()) as u32;
        idx += 2;

        let capability_flags = cap_lower | (cap_upper << 16);

        let auth_data_len = payload[idx];
        idx += 1;

        // 10 bytes reserved
        idx += 10;

        let mut auth_plugin_name = None;
        let has_secure = (capability_flags & capabilities::CLIENT_SECURE_CONNECTION) != 0;
        let has_plugin_auth = (capability_flags & capabilities::CLIENT_PLUGIN_AUTH) != 0;

        if has_secure || has_plugin_auth || auth_data_len > 8 {
            let part2_len = if auth_data_len > 8 {
                (auth_data_len as usize - 8).max(13)
            } else {
                13
            };
            if payload.len() >= idx + part2_len {
                auth_data.extend_from_slice(&payload[idx..idx + part2_len]);
                idx += part2_len;
            }
        }

        if (capability_flags & capabilities::CLIENT_PLUGIN_AUTH) != 0 && idx < payload.len() {
            let p_end = payload[idx..].iter().position(|&b| b == 0).unwrap_or(payload.len() - idx);
            auth_plugin_name = Some(String::from_utf8_lossy(&payload[idx..idx + p_end]).into_owned());
        }

        Ok(Self {
            protocol_version,
            server_version,
            connection_id,
            auth_plugin_data: auth_data,
            capability_flags,
            character_set,
            status_flags,
            auth_plugin_name,
        })
    }
}

/// Handshake Response 41 Packet sent by client during connection initialization.
pub struct HandshakeResponse41<'a> {
    pub client_capabilities: u32,
    pub max_packet_size: u32,
    pub character_set: u8,
    pub username: &'a str,
    pub auth_response: &'a [u8],
    pub database: Option<&'a str>,
    pub auth_plugin_name: Option<&'a str>,
}

impl<'a> HandshakeResponse41<'a> {
    pub fn serialize(&self) -> RawPacket {
        let mut payload = Vec::new();

        // 4 bytes client capabilities
        payload.extend_from_slice(&self.client_capabilities.to_le_bytes());
        // 4 bytes max packet size
        payload.extend_from_slice(&self.max_packet_size.to_le_bytes());
        // 1 byte character set (45 = utf8mb4_general_ci)
        payload.push(self.character_set);
        // 23 bytes reserved zeros
        payload.extend_from_slice(&[0u8; 23]);

        // null-terminated username
        payload.extend_from_slice(self.username.as_bytes());
        payload.push(0);

        // auth data length-encoded or null-terminated
        if (self.client_capabilities & capabilities::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA) != 0 {
            payload.push(self.auth_response.len() as u8);
            payload.extend_from_slice(self.auth_response);
        } else if (self.client_capabilities & capabilities::CLIENT_SECURE_CONNECTION) != 0 {
            payload.push(self.auth_response.len() as u8);
            payload.extend_from_slice(self.auth_response);
        } else {
            payload.extend_from_slice(self.auth_response);
            payload.push(0);
        }

        // null-terminated database
        if let Some(db) = self.database {
            if (self.client_capabilities & capabilities::CLIENT_CONNECT_WITH_DB) != 0 {
                payload.extend_from_slice(db.as_bytes());
                payload.push(0);
            }
        }

        // null-terminated auth plugin name
        if let Some(plugin) = self.auth_plugin_name {
            if (self.client_capabilities & capabilities::CLIENT_PLUGIN_AUTH) != 0 {
                payload.extend_from_slice(plugin.as_bytes());
                payload.push(0);
            }
        }

        RawPacket::new(1, payload)
    }
}

/// Command packet factory for standard MySQL command types.
pub struct CommandPacket;

impl CommandPacket {
    pub const COM_QUIT: u8 = 0x01;
    pub const COM_QUERY: u8 = 0x03;
    pub const COM_PING: u8 = 0x0E;

    pub fn query(sql: &str) -> RawPacket {
        let mut payload = Vec::with_capacity(1 + sql.len());
        payload.push(Self::COM_QUERY);
        payload.extend_from_slice(sql.as_bytes());
        RawPacket::new(0, payload)
    }

    pub fn ping() -> RawPacket {
        RawPacket::new(0, vec![Self::COM_PING])
    }

    pub fn quit() -> RawPacket {
        RawPacket::new(0, vec![Self::COM_QUIT])
    }
}

/// Generic MySQL server response indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerResponse {
    Ok { affected_rows: u64, last_insert_id: u64, status_flags: u16, warnings: u16 },
    Error { code: u16, sql_state: String, message: String },
    Eof { warnings: u16, status_flags: u16 },
    ResultSetHeader { column_count: u64 },
}

impl ServerResponse {
    pub fn parse(packet: &RawPacket) -> Result<Self, WireError> {
        let p = &packet.payload;
        if p.is_empty() {
            return Err(WireError::BufferTooShort);
        }

        match p[0] {
            0x00 => {
                // OK Packet
                let mut idx = 1;
                let (affected_rows, read) = read_lenenc_int(&p[idx..])?;
                idx += read;
                let (last_insert_id, read) = read_lenenc_int(&p[idx..])?;
                idx += read;
                let status_flags = if p.len() >= idx + 2 {
                    u16::from_le_bytes(p[idx..idx + 2].try_into().unwrap())
                } else {
                    0
                };
                idx += 2;
                let warnings = if p.len() >= idx + 2 {
                    u16::from_le_bytes(p[idx..idx + 2].try_into().unwrap())
                } else {
                    0
                };
                Ok(Self::Ok { affected_rows, last_insert_id, status_flags, warnings })
            }
            0xFF => {
                // ERR Packet: 0xFF + 2 bytes code + 1 byte sql_state_marker (#) + 5 bytes sql_state + message
                if p.len() < 3 {
                    return Err(WireError::BufferTooShort);
                }
                let code = u16::from_le_bytes(p[1..3].try_into().unwrap());
                let (sql_state, message) = if p.len() >= 9 && p[3] == b'#' {
                    let state = String::from_utf8_lossy(&p[4..9]).into_owned();
                    let msg = String::from_utf8_lossy(&p[9..]).into_owned();
                    (state, msg)
                } else {
                    ("HY000".to_string(), String::from_utf8_lossy(&p[3..]).into_owned())
                };
                Ok(Self::Error { code, sql_state, message })
            }
            0xFE if p.len() < 9 => {
                // EOF Packet
                let warnings = if p.len() >= 3 {
                    u16::from_le_bytes(p[1..3].try_into().unwrap())
                } else {
                    0
                };
                let status_flags = if p.len() >= 5 {
                    u16::from_le_bytes(p[3..5].try_into().unwrap())
                } else {
                    0
                };
                Ok(Self::Eof { warnings, status_flags })
            }
            _ => {
                // Resultset Column Count (length-encoded integer)
                let (col_count, _) = read_lenenc_int(p)?;
                Ok(Self::ResultSetHeader { column_count: col_count })
            }
        }
    }
}

/// Decodes MySQL Length-Encoded Integer (Protocol::LengthEncodedInteger).
pub fn read_lenenc_int(bytes: &[u8]) -> Result<(u64, usize), WireError> {
    if bytes.is_empty() {
        return Err(WireError::BufferTooShort);
    }
    match bytes[0] {
        0..=0xFA => Ok((bytes[0] as u64, 1)),
        0xFB => Err(WireError::ProtocolMismatch("null length-encoded integer".into())),
        0xFC => {
            if bytes.len() < 3 {
                return Err(WireError::BufferTooShort);
            }
            let val = u16::from_le_bytes(bytes[1..3].try_into().unwrap()) as u64;
            Ok((val, 3))
        }
        0xFD => {
            if bytes.len() < 4 {
                return Err(WireError::BufferTooShort);
            }
            let val = (bytes[1] as u64) | ((bytes[2] as u64) << 8) | ((bytes[3] as u64) << 16);
            Ok((val, 4))
        }
        0xFE => {
            if bytes.len() < 9 {
                return Err(WireError::BufferTooShort);
            }
            let val = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
            Ok((val, 9))
        }
        0xFF => Err(WireError::UnexpectedPacketType(0xFF)),
    }
}

/// Decodes Length-Encoded String from payload.
pub fn read_lenenc_string(bytes: &[u8]) -> Result<(String, usize), WireError> {
    let (len, read) = read_lenenc_int(bytes)?;
    let total = read + len as usize;
    if bytes.len() < total {
        return Err(WireError::BufferTooShort);
    }
    let s = String::from_utf8_lossy(&bytes[read..total]).into_owned();
    Ok((s, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_header_round_trip() {
        let h = PacketHeader { payload_length: 12345, sequence_id: 2 };
        let enc = h.encode();
        let dec = PacketHeader::decode(&enc).unwrap();
        assert_eq!(h, dec);
    }

    #[test]
    fn raw_packet_serialize_and_parse() {
        let p = RawPacket::new(1, vec![1, 2, 3, 4, 5]);
        let bytes = p.serialize();
        assert_eq!(bytes.len(), 4 + 5);

        let (parsed, read) = RawPacket::parse(&bytes).unwrap().unwrap();
        assert_eq!(read, 9);
        assert_eq!(parsed.sequence_id, 1);
        assert_eq!(parsed.payload, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn command_packets() {
        let q = CommandPacket::query("SELECT 1");
        assert_eq!(q.sequence_id, 0);
        assert_eq!(q.payload[0], CommandPacket::COM_QUERY);
        assert_eq!(&q.payload[1..], b"SELECT 1");

        let p = CommandPacket::ping();
        assert_eq!(p.payload, vec![CommandPacket::COM_PING]);
    }

    #[test]
    fn server_response_ok_and_err() {
        // Mock OK packet: 0x00, affected=1, insert_id=0, status=2, warnings=0
        let ok_pkt = RawPacket::new(1, vec![0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00]);
        let res = ServerResponse::parse(&ok_pkt).unwrap();
        match res {
            ServerResponse::Ok { affected_rows, last_insert_id, .. } => {
                assert_eq!(affected_rows, 1);
                assert_eq!(last_insert_id, 0);
            }
            _ => panic!("expected OK packet"),
        }

        // Mock ERR packet: 0xFF, code 1045, #28000 Access denied
        let mut err_payload = vec![0xFF, 0x15, 0x04]; // code 1045
        err_payload.push(b'#');
        err_payload.extend_from_slice(b"28000");
        err_payload.extend_from_slice(b"Access denied");
        let err_pkt = RawPacket::new(1, err_payload);
        let err_res = ServerResponse::parse(&err_pkt).unwrap();
        match err_res {
            ServerResponse::Error { code, sql_state, message } => {
                assert_eq!(code, 1045);
                assert_eq!(sql_state, "28000");
                assert_eq!(message, "Access denied");
            }
            _ => panic!("expected Error packet"),
        }
    }

    #[test]
    fn handshake_v10_parse() {
        let mut payload = vec![10]; // protocol v10
        payload.extend_from_slice(b"8.4.0\0"); // server version
        payload.extend_from_slice(&1234u32.to_le_bytes()); // conn id
        payload.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]); // auth 1
        payload.push(0); // filler
        let cap_lower = (capabilities::CLIENT_PROTOCOL_41 | capabilities::CLIENT_SECURE_CONNECTION) as u16;
        payload.extend_from_slice(&cap_lower.to_le_bytes()); // cap lower
        payload.push(45); // charset utf8mb4
        payload.extend_from_slice(&0x0002u16.to_le_bytes()); // status AUTOCOMMIT
        let cap_upper = (capabilities::CLIENT_PLUGIN_AUTH >> 16) as u16;
        payload.extend_from_slice(&cap_upper.to_le_bytes()); // cap upper
        payload.push(21); // auth data len
        payload.extend_from_slice(&[0u8; 10]); // reserved
        payload.extend_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21]); // auth 2
        payload.extend_from_slice(b"caching_sha2_password\0");

        let hs = HandshakeV10::parse(&payload).unwrap();
        assert_eq!(hs.protocol_version, 10);
        assert_eq!(hs.server_version, "8.4.0");
        assert_eq!(hs.connection_id, 1234);
        assert_eq!(hs.character_set, 45);
        assert_eq!(hs.auth_plugin_name.as_deref(), Some("caching_sha2_password"));
    }
}
