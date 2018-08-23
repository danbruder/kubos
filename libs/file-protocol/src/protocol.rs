//
// Copyright (C) 2018 Kubos Corporation
//
// Licensed under the Apache License, Version 2.0 (the "License")
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

//! TODO: Module documentation

use super::messages;
use super::parsers;
use super::storage;
use super::Message;
use cbor_protocol::Protocol as CborProtocol;
use serde_cbor::{ser, Value};
use std::cell::Cell;
use std::net::UdpSocket;
use std::str;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// File Transfer Protocol Roles
#[derive(Eq, PartialEq)]
pub enum Role {
    /// User will be sending operation requests
    Client,
    /// User will be listening for and processing operation requests
    Server,
}

/// File Protocol Information Structure
pub struct Protocol {
    cbor_proto: CborProtocol,
    host: String,
    dest_port: Cell<u16>,
    role: Role,
}

impl Protocol {
    /// Create a new file protocol instance using an automatically assigned UDP socket
    pub fn new(host: String, dest_port: u16, role: Role) -> Self {
        // Get a local UDP socket (Bind)
        let c_protocol = CborProtocol::new(format!("{}:0", host));

        // Set up the full connection info
        Protocol {
            cbor_proto: c_protocol,
            // Remote IP?
            host,
            dest_port: Cell::new(dest_port),
            role,
        }
    }

    /// Create a new file protocol instance using a specific UDP socket
    pub fn new_from_socket(socket: UdpSocket, host: String, dest_port: u16, role: Role) -> Self {
        Protocol {
            cbor_proto: CborProtocol::new_from_socket(socket),
            host,
            dest_port: Cell::new(dest_port),
            role,
        }
    }

    /// Wrap the specified data into a CBOR packet and then send to the destination port
    pub fn send(&self, vec: Vec<u8>) -> Result<(), String> {
        self.cbor_proto
            .send_message(&vec, &self.host, self.dest_port.get())
            .unwrap();
        Ok(())
    }

    /// Request remote target to receive file from host
    pub fn send_export(&self, hash: &str, target_path: &str, mode: u32) -> Result<(), String> {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .and_then(
                |duration| Ok(duration.as_secs() * 1000 + duration.subsec_nanos() as u64 / 1000000),
            )
            .map_err(|err| format!("Failed to get current system time: {}", err))?;
        let channel_id: u32 = (time % 100000) as u32;

        self.send(messages::export_request(channel_id, hash, target_path, mode).unwrap())
            .unwrap();

        Ok(())
    }

    /// Request a file from a remote target
    pub fn send_import(&self, source_path: &str) -> Result<(), String> {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .and_then(
                |duration| Ok(duration.as_secs() * 1000 + duration.subsec_nanos() as u64 / 1000000),
            )
            .map_err(|err| format!("Failed to get current system time: {}", err))?;
        let channel_id: u32 = (time % 100000) as u32;

        self.send(messages::import_request(channel_id, source_path).unwrap())
            .unwrap();
        Ok(())
    }

    /// Figure out if/what chunks are missing and send the hash and info back to the remote addr
    pub fn sync_and_send(&self, hash: &str, num_chunks: Option<u32>) -> Result<(), String> {
        // TODO: Create some way to break out of this loop if we never receive all the chunks
        loop {
            let (result, _chunks) = storage::validate_file(hash, num_chunks)?;

            self.send(messages::file_status(hash, num_chunks).unwrap())
                .unwrap();

            if result == true {
                // We've received all the chunks we were expecting. Time to go home.
                break;
            }

            // Try to receive the missing chunks
            loop {
                // Listen on UDP port
                // TODO: Make timeout a config option
                // TODO: Make timeout 'receive chunk' message-specific
                match self.cbor_proto.recv_message_timeout(Duration::from_secs(1)) {
                    // Parse the received message
                    Ok(Some(message)) => match self.process_message(message, Some(hash)) {
                        Ok(_) => { /* TODO: Verify that we got a ReceiveChunk message? */ }
                        Err(err) => eprintln!("Failed to parse message: {}", err),
                    },
                    Ok(None) => { /* TODO: Handle pause or resume messages? */ }
                    Err(None) => {
                        // We timed out of receiving a new chunk. Let's go see if we got everything
                        break;
                    }
                    Err(Some(err)) => {
                        // Something went wrong while we were receiving
                        // Let's quit while we're ahead
                        return Err(err);
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify the integrity of received file data and then transfer into the requested permanent file location
    ///
    /// Verifies:
    /// 	a) All of the chunks of a file have been received
    ///     b) That the calculated hash of said chunks matches the expected hash
    ///
    pub fn finalize_file(
        &self,
        hash: &str,
        target_path: &str,
        mode: Option<u32>,
    ) -> Result<(), String> {
        storage::finalize_file(hash, target_path, mode)
    }

    /// Store a files metadata into the appropriate temporary storage location
    pub fn store_meta(&self, hash: &str, num_chunks: u32) -> Result<(), String> {
        storage::store_meta(hash, num_chunks)
    }

    /// Send all requested chunks of a file to the remote destination
    pub fn send_chunks(&self, hash: &str, chunks: &[(u32, u32)]) -> Result<(), String> {
        for (first, last) in chunks {
            for chunk_index in *first..*last {
                let chunk = storage::load_chunk(hash, chunk_index).unwrap();
                self.send(messages::chunk(hash, chunk_index, &chunk).unwrap())
                    .unwrap();
            }
        }
        Ok(())
    }

    /// Report to the requestor that the export operation has completed successfully
    pub fn send_success(&self, channel_id: u64) -> Result<(), String> {
        info!("-> {{ {}, true }}", channel_id);
        let vec = ser::to_vec_packed(&(channel_id, true)).unwrap();
        self.cbor_proto
            .send_message(&vec, &self.host, self.dest_port.get())
            .unwrap();
        Ok(())
    }

    /// Report to the requestor that the export operation has failed
    pub fn send_failure(&self, channel_id: u64, error: &str) -> Result<(), String> {
        info!("-> {{ {}, false, {} }}", channel_id, error);
        let vec = ser::to_vec_packed(&(channel_id, false, error)).unwrap();
        self.cbor_proto
            .send_message(&vec, &self.host, self.dest_port.get())
            .unwrap();
        Ok(())
    }

    /// Listen for and process file protocol messages
    /// TODO: Make this more descriptive
    pub fn message_engine(
        &self,
        hash: Option<&str>,
        timeout: Duration,
        pump: bool,
    ) -> Result<Option<Message>, String> {
        let mut last_message: Result<Option<Message>, String>;
        loop {
            // Listen on UDP port
            info!("listening...");

            let message = if self.role == Role::Client {
                match self.cbor_proto.recv_message_peer_timeout(timeout)? {
                    (peer, Some(message)) => {
                        // Update our response port
                        self.dest_port.set(peer.port());
                        message
                    }
                    _ => return Err("Failed to receive op result".to_owned()),
                }
            } else {
                match self.cbor_proto.recv_message_timeout(timeout) {
                    Ok(Some(message)) => message,
                    _ => return Err("Failed to receive data".to_owned()),
                }
            };

            let new_message = self.process_message(message, hash);

            let stop = match new_message.to_owned() {
                Ok(Some(Message::ACK(_))) => true,
                Ok(Some(Message::SuccessReceive(_))) => true,
                Ok(Some(Message::SuccessTransmit(_, _, _, _))) => true,
                Ok(Some(_message)) => {
                    if !pump {
                        true
                    } else {
                        false
                    }
                }
                Ok(None) => false,
                Err(e) => return Err(e),
            };
            last_message = new_message;
            if stop {
                break;
            }
        }
        last_message
    }

    /// Process a file protocol message
    pub fn process_message(
        &self,
        message: Value,
        hash: Option<&str>,
    ) -> Result<Option<Message>, String> {
        let parsed_message = parsers::parse_message(message);
        //println!("parsed_message: {:?}", parsed_message);
        match parsed_message.to_owned() {
            Ok(Message::Sync(hash)) => {
                info!("<- {{ {} }}", hash);
            }
            Ok(Message::Metadata(hash, num_chunks)) => {
                info!("<- {{ {}, {} }}", hash, num_chunks);
                storage::store_meta(&hash, num_chunks).unwrap();
            }
            Ok(Message::ReceiveChunk(hash, chunk_num, data)) => {
                info!("<- {{ {}, {}, chunk_data }}", hash, chunk_num);
                storage::store_chunk(&hash, chunk_num, &data).unwrap();
            }
            Ok(Message::ACK(ack_hash)) => {
                info!("<- {{ {}, true }}", ack_hash);
                if let Some(hash_val) = hash {
                    if ack_hash == hash_val {
                        // Done processing its time to go home
                        // return Ok(true);
                    }
                }
                // return Ok(true);
            }
            Ok(Message::NAK(hash, Some(missing_chunks))) => {
                info!("<- {{ {}, false, {:?} }}", hash, missing_chunks);
                self.send_chunks(&hash, &missing_chunks)?;
            }
            Ok(Message::NAK(hash, None)) => {
                info!("<- {{ {}, false }}", hash);
            }
            Ok(Message::ReqReceive(channel_id, hash, path, Some(mode))) => {
                info!(
                    "<- {{ {}, export, {}, {}, {} }}",
                    channel_id, hash, path, mode
                );
                // The client wants to send us a file.
                // Go listen for the chunks.
                // Note: Won't return until we've received all of them.
                // (so could potentially never return)
                // TODO: handle channel_id mismatch
                let (_result, _chunks) = storage::validate_file(&hash, None).unwrap();
                self.send(messages::file_status(&hash, None).unwrap())
                    .unwrap();
            }
            Ok(Message::ReqReceive(channel_id, hash, path, None)) => {
                info!("<- {{ {}, export, {}, {} }}", channel_id, hash, path);
            }
            Ok(Message::ReqTransmit(channel_id, path)) => {
                info!("<- {{ {}, import, {} }}", channel_id, path);
                self.send(messages::import_result(channel_id, &path).unwrap())
                    .unwrap();
            }
            Ok(Message::SuccessReceive(channel_id)) => {
                info!("<- {{ {}, true }}", channel_id);
            }
            Ok(Message::SuccessTransmit(channel_id, hash, num_chunks, Some(mode))) => {
                info!(
                    "<- {{ {}, true, {}, {}, {} }}",
                    channel_id, hash, num_chunks, mode
                );
                // TODO: handle channel_id mismatch
                let (_result, _chunks) = storage::validate_file(&hash, Some(num_chunks)).unwrap();
                self.send(messages::file_status(&hash, Some(num_chunks)).unwrap())
                    .unwrap();
            }
            Ok(Message::SuccessTransmit(channel_id, hash, num_chunks, None)) => {
                info!("<- {{ {}, true, {}, {} }}", channel_id, hash, num_chunks);
                // TODO: handle channel_id mismatch
                let (_result, _chunks) = storage::validate_file(&hash, Some(num_chunks)).unwrap();
                self.send(messages::file_status(&hash, Some(num_chunks)).unwrap())
                    .unwrap();
            }
            Ok(Message::Failure(channel_id, error_message)) => {
                info!("<- {{ {}, false, {} }}", channel_id, error_message);
                return Err(format!(
                    "Transmission failure on channel {}. Error returned from server: {}",
                    channel_id, error_message
                ));
            }
            Err(e) => {
                info!("<- what did we get?? {}", e);
            }
        }

        if parsed_message.is_ok() {
            return Ok(Some(parsed_message.unwrap()));
        } else {
            return Ok(None);
        }
    }
}