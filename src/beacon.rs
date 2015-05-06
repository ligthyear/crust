// Copyright 2015 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under (1) the MaidSafe.net Commercial License,
// version 1.0 or later, or (2) The General Public License (GPL), version 3, depending on which
// licence you accepted on initial access to the Software (the "Licences").
//
// By contributing code to the SAFE Network Software, or to this project generally, you agree to be
// bound by the terms of the MaidSafe Contributor Agreement, version 1.0.  This, along with the
// Licenses can be found in the root directory of this project at LICENSE, COPYING and CONTRIBUTOR.
//
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.
//
// Please review the Licences for the specific language governing permissions and limitations
// relating to use of the SAFE Network Software.

use rand::random;
use std::io::Result;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::{Arc, mpsc, Mutex};
use std::thread;

use transport;
use transport::{Acceptor, Port, Transport};

const GUID_SIZE: usize = 16;
const MAGIC_SIZE: usize = 4;
const MAGIC: [u8; MAGIC_SIZE] = ['m' as u8, 'a' as u8, 'i' as u8, 'd' as u8];

pub type GUID = [u8; GUID_SIZE];

pub fn serialise_address(our_listening_address: SocketAddr) -> [u8; 27] {
    let mut our_details = [0u8; 27];
    match our_listening_address {
        SocketAddr::V4(ref v4_address) => {
            // Leave first byte as 0 to indicate IPv4
            for i in 0..4 {
                our_details[i + 1] = v4_address.ip().octets()[i];
            }
            our_details[5] = (v4_address.port() >> 8) as u8;
            our_details[6] = v4_address.port() as u8;
        },
        SocketAddr::V6(ref v6_address) => {
            // Set first byte as 1 to indicate IPv6
            our_details[0] = 1u8;
            for i in 0..8 {
                our_details[(2 * i) + 1] = (v6_address.ip().segments()[i] >> 8) as u8;
                our_details[(2 * i) + 2] = v6_address.ip().segments()[i] as u8;
            }
            our_details[17] = (v6_address.port() >> 8) as u8;
            our_details[18] = v6_address.port() as u8;
            our_details[19] = (v6_address.flowinfo() >> 24) as u8;
            our_details[20] = (v6_address.flowinfo() >> 16) as u8;
            our_details[21] = (v6_address.flowinfo() >> 8) as u8;
            our_details[22] = v6_address.flowinfo() as u8;
            our_details[23] = (v6_address.scope_id() >> 24) as u8;
            our_details[24] = (v6_address.scope_id() >> 16) as u8;
            our_details[25] = (v6_address.scope_id() >> 8) as u8;
            our_details[26] = v6_address.scope_id() as u8;
        },
    }
    our_details
}

pub fn parse_address(buffer: &[u8]) -> Option<SocketAddr> {
    match buffer[0] {
        0 => {
            let port: u16 = ((buffer[5] as u16) * 256) + (buffer[6] as u16);
            let peer_socket = SocketAddrV4::new(Ipv4Addr::new(
                buffer[1], buffer[2], buffer[3], buffer[4]), port);
            // println!("Received IPv4 address {:?}\n", peer_socket);
            Some(SocketAddr::V4(peer_socket))
        },
        1 => {
            let mut segments = [0u16; 8];
            for i in 0..8 {
                segments[i] =
                    ((buffer[(2 * i) + 1] as u16) << 8) + (buffer[(2 * i) + 2] as u16);
            }
            let port: u16 = ((buffer[17] as u16) << 8) + (buffer[18] as u16);
            let flowinfo: u32 =
                ((buffer[19] as u32) << 24) + ((buffer[20] as u32) << 16) +
                ((buffer[21] as u32) << 8) + (buffer[22] as u32);
            let scope_id: u32 =
                ((buffer[23] as u32) << 24) + ((buffer[24] as u32) << 16) +
                ((buffer[25] as u32) << 8) + (buffer[26] as u32);
            let peer_socket = SocketAddrV6::new(Ipv6Addr::new(
                segments[0], segments[1], segments[2], segments[3], segments[4],
                segments[5], segments[6], segments[7]), port, flowinfo, scope_id);
            // println!("Received IPv6 address {:?} with flowinfo {} and scope_id {}\n",
            //           peer_socket, flowinfo, scope_id);
            Some(SocketAddr::V6(peer_socket))
        },
        _ => None,
    }
}

fn serialise_port(port: u16) -> [u8;2] {
    [(port & 0xff) as u8, (port >> 8) as u8]
}

fn parse_port(data: [u8;2]) -> u16 {
    (data[0] as u16) + ((data[1] as u16) << 8)
}

pub struct BroadcastAcceptor {
    guid: GUID,
    socket: UdpSocket,
    acceptor: Arc<Mutex<Acceptor>>,
}

impl BroadcastAcceptor {
    pub fn new(port: u16) -> Result<BroadcastAcceptor> {
        let socket = try!(UdpSocket::bind(("0.0.0.0", port)));
        let acceptor = try!(transport::new_acceptor(&Port::Tcp(0)));
        let mut guid = [0; GUID_SIZE];
        for i in 0..GUID_SIZE {
            guid[i] = random::<u8>();
        }
        Ok(BroadcastAcceptor{ guid: guid,
                              socket: socket,
                              acceptor: Arc::new(Mutex::new(acceptor)) })
    }

    // FIXME: Proper error handling and cancelation.
    pub fn accept(&self) -> Result<Transport> {
        let (port_sender, port_receiver) = mpsc::channel::<u16>();
        let (transport_sender, transport_receiver) = mpsc::channel::<Transport>();

        let protected_acceptor = self.acceptor.clone();
        let run_acceptor = move || -> Result<()> {
            let acceptor = protected_acceptor.lock().unwrap();
            let _ = port_sender.send(acceptor.local_endpoint().get_address().port());
            let transport = try!(transport::accept(&acceptor));
            let _ = transport_sender.send(transport);
            Ok(())
        };
        let t1 = thread::scoped(move || { let _ = run_acceptor(); });

        let tcp_port = port_receiver.recv().unwrap(); // We don't expect this to fail.

        let run_listener = move || -> Result<()> {
            let mut buffer = vec![0u8; MAGIC_SIZE + GUID_SIZE];
            loop {
                let (_, source) = try!(self.socket.recv_from(&mut buffer[..]));
                if buffer[0..MAGIC_SIZE] != MAGIC { continue; }
                if buffer[MAGIC_SIZE..(MAGIC_SIZE+GUID_SIZE)] == self.guid { continue; }
                let reply_socket = try!(UdpSocket::bind("0.0.0.0:0"));
                let sent_size = try!(reply_socket.send_to(&serialise_port(tcp_port), source));
                debug_assert!(sent_size == 2);
                break;
            }
            Ok(())
        };
        let t2 = thread::scoped(move || { let _ = run_listener(); });

        let _ = t1.join();
        let _ = t2.join();

        Ok(transport_receiver.recv().unwrap())
    }

    pub fn beacon_port(&self) -> u16 {
        match self.socket.local_addr() {
            Ok(address) => address.port(),
            Err(_) => 0u16,
        }
    }

    pub fn beacon_guid(&self) -> GUID {
        self.guid
    }
}

pub fn seek_peers(port: u16, guid_to_avoid: Option<GUID>) -> Result<Vec<SocketAddr>> {
    // Send broadcast ping
    let socket = try!(UdpSocket::bind("0.0.0.0:0"));
    try!(socket.set_broadcast(true));

    let mut send_buff = Vec::<u8>::with_capacity(MAGIC_SIZE + GUID_SIZE);
    for c in MAGIC.iter() { send_buff.push(c.clone()); }
    let guid = guid_to_avoid.unwrap_or([0; GUID_SIZE]);
    for c in guid.iter() { send_buff.push(c.clone()); }

    let sent_size = try!(socket.send_to(&send_buff[..], ("255.255.255.255", port)));
    debug_assert!(sent_size == send_buff.len());

    let (tx, rx) = mpsc::channel::<SocketAddr>();

    // FIXME: This thread will never finish, eating one udp port
    // and few resources till the end of the program. I haven't
    // found a way to fix this in rust yet.
    let runner = move || -> Result<()> {
        let mut buffer = [0u8; 2];
        let (_, source) = try!(socket.recv_from(&mut buffer));
        let his_port = parse_port(buffer);
        let his_ep = SocketAddr::new(source.ip(), his_port);
        let _ = tx.send(his_ep);
        Ok(())
    };

    let _ = thread::spawn(move || { let _ = runner(); });

    // Allow peers to respond.
    thread::sleep_ms(500);

    let mut result = Vec::<SocketAddr>::new();

    loop {
        match rx.try_recv() {
            Ok(socket_addr) => result.push(socket_addr),
            Err(_) => break,
        }
    }

    Ok(result)
}

#[test]
fn test_beacon() {
    let acceptor = BroadcastAcceptor::new(0).unwrap();
    let acceptor_port = acceptor.beacon_port();

    let t1 = thread::spawn(move || {
        let mut transport = acceptor.accept().unwrap();
        transport.sender.send(&"hello beacon".to_string().into_bytes()).unwrap();
    });

    let t2 = thread::spawn(move || {
        let endpoint = seek_peers(acceptor_port, None).unwrap()[0];
        let transport = transport::connect(transport::Endpoint::Tcp(endpoint)).unwrap();
        let msg = String::from_utf8(transport.receiver.receive().unwrap()).unwrap();
        assert!(msg == "hello beacon".to_string());
    });

    assert!(t1.join().is_ok());
    assert!(t2.join().is_ok());
}

#[test]
fn test_avoid_beacon() {
    let acceptor = BroadcastAcceptor::new(0).unwrap();
    let acceptor_port = acceptor.beacon_port();
    let my_guid = acceptor.guid.clone();

    let t1 = thread::spawn(move || {
        let _ = acceptor.accept().unwrap();
    });

    let t2 = thread::spawn(move || {
        assert!(seek_peers(acceptor_port, Some(my_guid)).unwrap().len() == 0);
    });

    // This one is just so that the first thread breaks.
    let t3 = thread::spawn(move || {
        thread::sleep_ms(700);
        let endpoint = seek_peers(acceptor_port, None).unwrap()[0];
        let _ = transport::connect(transport::Endpoint::Tcp(endpoint)).unwrap();
    });

    assert!(t1.join().is_ok());
    assert!(t2.join().is_ok());
    assert!(t3.join().is_ok());
}
