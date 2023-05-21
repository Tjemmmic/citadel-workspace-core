
use citadel_logging::info;
use citadel_sdk::prelude::*;
use citadel_workspace_types::InternalServicePayload;
use futures::stream::StreamExt;
use futures::SinkExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

pub struct CitadelWorkspaceService {
    pub remote: Option<NodeRemote>,
    // 127.0.0.1:55555
    pub bind_address: SocketAddr,
}

#[async_trait]
impl NetKernel for CitadelWorkspaceService {
    fn load_remote(&mut self, node_remote: NodeRemote) -> Result<(), NetworkError> {
        self.remote = Some(node_remote);
        Ok(())
    }

    async fn on_start(&self) -> Result<(), NetworkError> {
        let mut remote = self.remote.clone().unwrap();
        let listener = tokio::net::TcpListener::bind(self.bind_address).await?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<InternalServicePayload>();

        let hm = &Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let listener_task = async move {
            while let Ok((conn, _addr)) = listener.accept().await {
                let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel::<InternalServicePayload>();
                let id = Uuid::new_v4();
                hm.lock().await.insert(id, tx1);
                handle_connection(conn, tx.clone(), rx1, id);
            }
            Ok(())
        };

        let mut connection_map: HashMap<u64, PeerChannelSendHalf> = HashMap::new();

        let inbound_command_task = async move {
            while let Some(command) = rx.recv().await {
                payload_handler(command, &mut connection_map, &mut remote, hm).await;
            }
            Ok(())
        };

        tokio::select! {
            res0 = listener_task => res0,
            res1 = inbound_command_task => res1,
        }
    }

    async fn on_node_event_received(&self, _message: NodeResult) -> Result<(), NetworkError> {
        // TODO: handle disconnect properly by removing entries from the hashmap
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<(), NetworkError> {
        Ok(())
    }
}

async fn send_response_to_tcp_client(
    hash_map: &Arc<tokio::sync::Mutex<HashMap<Uuid, UnboundedSender<InternalServicePayload>>>>,
    response: InternalServicePayload,
    uuid: Uuid,
) {
    hash_map
        .lock()
        .await
        .get(&uuid)
        .unwrap()
        .send(response)
        .unwrap()
}

async fn payload_handler(
    command: InternalServicePayload,
    connection_map: &mut HashMap<u64, PeerChannelSendHalf>,
    remote: &mut NodeRemote,
    hm: &Arc<tokio::sync::Mutex<HashMap<Uuid, UnboundedSender<InternalServicePayload>>>>,
) {
    match command {
        InternalServicePayload::Connect {
            uuid,
            username,
            password,
        } => {
            // TODO: make sure register before connect.
            match remote
                .connect_with_defaults(AuthenticationRequest::credentialed(username, password))
                .await
            {
                //adde or self.bind_addr??
                Ok(conn_success) => {
                    let cid = conn_success.cid;

                    let (sink, mut stream) = conn_success.channel.split();
                    connection_map.insert(cid, sink);

                    let hm_for_conn = hm.clone();

                    // TODO: tell the client that the connection was successful
                    let response = InternalServicePayload::ConnectSuccess { cid };

                    send_response_to_tcp_client(hm, response, uuid).await;

                    let connection_read_stream = async move {
                        while let Some(message) = stream.next().await {
                            let message = InternalServicePayload::MessageReceived {
                                message: message.into_buffer(),
                                cid,
                                peer_cid: 0,
                            };
                            match hm_for_conn.lock().await.get(&uuid) {
                                Some(entry) => match entry.send(message) {
                                    Ok(res) => res,
                                    Err(_) => info!(target: "citadel", "tx not sent"),
                                },
                                None => {
                                    info!(target:"citadel","Hash map connection not found")
                                }
                            }
                        }
                    };
                    tokio::spawn(connection_read_stream);
                }

                Err(err) => {
                    let response = InternalServicePayload::ConnectionFailure {
                        message: err.to_string(),
                    };
                    send_response_to_tcp_client(hm, response, uuid).await;
                }
            };
        }
        InternalServicePayload::Register {
            uuid,
            server_addr,
            full_name,
            username,
            proposed_password,
        } => {
            citadel_logging::info!(target: "citadel", "About to connect to server {server_addr:?} for user {username}");
            match remote
                .register_with_defaults(server_addr, full_name, username, proposed_password)
                .await
            {
                Ok(_res) => {
                    // TODO: add trace ID to ensure uniqueness of request
                    let response = InternalServicePayload::RegisterSuccess { id: uuid };
                    send_response_to_tcp_client(hm, response, uuid).await
                }
                Err(err) => {
                    let response = InternalServicePayload::RegisterFailure {
                        message: err.to_string(),
                    };
                    send_response_to_tcp_client(hm, response, uuid).await
                }
            };
        }
        InternalServicePayload::Message {
            message,
            cid,
            security_level,
        } => {
            match connection_map.get_mut(&cid) {
                Some(sink) => {
                    sink.set_security_level(security_level);
                    sink.send_message(message.into()).await.unwrap();
                }
                None => info!(target: "citadel","connection not found"),
            };
        }
        InternalServicePayload::ConnectionFailure { .. } => {}
        InternalServicePayload::MessageReceived { .. } => {}
        InternalServicePayload::Disconnect { .. } => {}
        InternalServicePayload::SendFile { .. } => {}
        InternalServicePayload::DownloadFile { .. } => {}
        InternalServicePayload::ServiceConnectionAccepted { .. } => {}
        InternalServicePayload::ConnectSuccess { cid: _ } => {}
        InternalServicePayload::RegisterSuccess { .. } => {}
        InternalServicePayload::RegisterFailure { .. } => todo!(),
    }
}

pub fn wrap_tcp_conn(conn: TcpStream) -> Framed<TcpStream, LengthDelimitedCodec> {
    LengthDelimitedCodec::builder()
        .length_field_offset(0) // default value
        .max_frame_length(1024 * 1024 * 64) // 64 MB
        .length_field_type::<u32>()
        .length_adjustment(0) // default value
        // `num_skip` is not needed, the default is to skip
        .new_framed(conn)
}

fn handle_connection(
    conn: tokio::net::TcpStream,
    to_kernel: tokio::sync::mpsc::UnboundedSender<InternalServicePayload>,
    mut from_kernel: tokio::sync::mpsc::UnboundedReceiver<InternalServicePayload>,
    conn_id: Uuid,
) {
    tokio::task::spawn(async move {
        let framed = wrap_tcp_conn(conn);
        let (mut sink, mut stream) = framed.split();

        let write_task = async move {
            let response = InternalServicePayload::ServiceConnectionAccepted { id: conn_id };
            match bincode2::serialize(&response) {
                Ok(res) => {
                    match sink.send(res.into()).await {
                        Ok(_) => (),
                        Err(_) => info!(target: "citadel", "w task: sink send err"),
                    };
                }
                Err(_) => info!(target: "citadel", "write task: serialization err"),
            };

            while let Some(kernel_response) = from_kernel.recv().await {
                match bincode2::serialize(&kernel_response) {
                    Ok(k_res) => {
                        match sink.send(k_res.into()).await {
                            Ok(_) => (),
                            Err(_) => info!(target: "citadel", "w task: sink send err"),
                        };
                    }
                    Err(_) => info!(target: "citadel", "write task: serialization err"),
                };
                
            }
        };

        let read_task = async move {
            while let Some(message) = stream.next().await {
                match bincode2::deserialize(&message.unwrap()) {
                    Ok(request) => {
                        match to_kernel.send(request) {
                            Ok(res) => res,
                            Err(_) => info!(target: "citadel", "r task: sink send err"),
                        };
                    }
                    Err(_) => info!(target: "citadel", "read task deserialization err"),
                }
                
            }
        };

        tokio::select! {
            res0 = write_task => res0,
            res1 = read_task => res1,
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    
    
    use futures::stream::SplitSink;
    use std::error::Error;
    use std::time::Duration;
    use tokio::net::TcpStream;

    async fn send(
        sink: &mut SplitSink<Framed<TcpStream, LengthDelimitedCodec>, Bytes>,
        command: InternalServicePayload,
    ) -> Result<(), Box<dyn Error>> {
        let command = bincode2::serialize(&command)?;
        // send the command
        sink.send(command.into()).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_citadel_workspace_service() -> Result<(), Box<dyn Error>> {
        citadel_logging::setup_log();
        info!(target: "citadel", "above server spawn");

        let bind_address_internal_service: SocketAddr = "127.0.0.1:55556".parse().unwrap();
        // TCP client (GUI, CLI) -> internal service -> empty kernel server(s)
        let (server, server_bind_address) = citadel_sdk::test_common::server_info();

        tokio::task::spawn(server);
        info!(target: "citadel", "sub server spawn");
        let internal_service_kernel = CitadelWorkspaceService {
            remote: None,
            bind_address: bind_address_internal_service,
        };
        let internal_service = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);

        // give time for both the server and internal service to run

        tokio::time::sleep(Duration::from_millis(2000)).await;

        info!(target: "citadel", "about to connect to internal service");

        // begin mocking the GUI/CLI access
        let conn = TcpStream::connect(bind_address_internal_service).await?;
        info!(target: "citadel", "connected to the TCP stream");
        let framed = wrap_tcp_conn(conn);
        info!(target: "citadel", "wrapped tcp connection");

        let (mut sink, mut stream) = framed.split();

        let first_packet = stream.next().await.unwrap()?;
        info!(target: "citadel", "First packet");
        let greeter_packet: InternalServicePayload = bincode2::deserialize(&first_packet)?;

        info!(target: "citadel", "Greeter packet {greeter_packet:?}");

        if let InternalServicePayload::ServiceConnectionAccepted { id } = greeter_packet {
            let register_command = InternalServicePayload::Register {
                uuid: id,
                server_addr: server_bind_address,
                full_name: String::from("John"),
                username: String::from("john_doe"),
                proposed_password: String::from("test12345").into_bytes().into(),
            };
            send(&mut sink, register_command).await?;

            let second_packet = stream.next().await.unwrap()?;
            let response_packet: InternalServicePayload = bincode2::deserialize(&second_packet)?;
            if let InternalServicePayload::RegisterSuccess { id } = response_packet {
                // now, connect to the server
                let command = InternalServicePayload::Connect {
                    // server_addr: server_bind_address,
                    username: String::from("john_doe"),
                    password: String::from("test12345").into_bytes().into(),
                    uuid: id,
                };

                send(&mut sink, command).await?;

                let next_packet = stream.next().await.unwrap()?;
                let response_packet: InternalServicePayload = bincode2::deserialize(&next_packet)?;
                if let InternalServicePayload::ConnectSuccess { cid: _ } = response_packet {
                    Ok(())
                } else {
                    panic!("Connection to server was not a success")
                }
            } else {
                panic!("Registration to server was not a success")
            }
        } else {
            panic!("Wrong packet type");
        }
    }
}
