use bytes::{Bytes, BytesMut};
use citadel_sdk::prelude::{ConnectMode, SecBuffer, SecurityLevel, SessionSecuritySettings, TransferType, UdpMode, UserIdentifier};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use uuid::Uuid;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InternalServiceResponse {
    ConnectSuccess {
        cid: u64,
    },
    ConnectionFailure {
        message: String,
    },
    RegisterSuccess {
        id: Uuid,
    },
    RegisterFailure {
        message: String,
    },
    ServiceConnectionAccepted {
        id: Uuid,
    },
    MessageSent {
        cid: u64,
        // TODO: investigate passing a message hash or a trace id
    },
    MessageSendError {
        cid: u64,
        message: String,
    },
    MessageReceived {
        message: BytesMut,
        cid: u64,
        peer_cid: u64,
    },
    DisconnectSuccess { cid: u64 },
    DisconnectFailure { cid: u64, message: String },
    SendFileSuccess { cid: u64 },
    SendFileFailure { cid: u64, message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InternalServicePayload {
    Connect {
        uuid: Uuid,
        username: String,
        password: SecBuffer,
        connect_mode: ConnectMode,
        udp_mode: UdpMode,
        keep_alive_timeout: Option<Duration>,
        session_security_settings: SessionSecuritySettings
    },
    Register {
        uuid: Uuid,
        server_addr: SocketAddr,
        full_name: String,
        username: String,
        proposed_password: SecBuffer,
        default_security_settings: SessionSecuritySettings
    },
    Message {
        uuid: Uuid,
        message: Bytes,
        cid: u64,
        user_cid: u64,
        security_level: SecurityLevel,
    },
    Disconnect {
        uuid: Uuid,
        cid: u64,
    },
    SendFile {
        uuid: Uuid,
        source: PathBuf,
        cid: u64,
        chunk_size: usize,
        transfer_type: TransferType
    },
    DownloadFile {
        virtual_path: PathBuf,
        transfer_security_level: SecurityLevel,
        delete_on_pull: bool,
        cid: u64,
        uuid: Uuid
    },
    StartGroup {
        initial_users_to_invite: Option<Vec<UserIdentifier>>,
        cid: u64,
        uuid: Uuid
    },
}
