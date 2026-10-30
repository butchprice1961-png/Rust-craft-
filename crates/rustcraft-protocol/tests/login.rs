//! Logs in to an in-process server that follows the 1.8 login sequence:
//! online mode with encryption (checked against a mock session service),
//! then compression, then play packets in both directions.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};

use rustcraft_protocol::codec::{Buf, Put};
use rustcraft_protocol::conn::{PacketReader, PacketWriter};
use rustcraft_protocol::login::{server_hash, status};
use rustcraft_protocol::packets::decode;
use rustcraft_protocol::{Clientbound, Error, Profile, Session, join, send};

/// Answers one HTTP request with 204 and returns its body.
fn mock_session_service() -> (String, thread::JoinHandle<String>) {
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/session/minecraft/join", http.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (stream, _) = http.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (&stream).write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        String::from_utf8(body).unwrap()
    });
    (url, handle)
}

fn accept(server: &TcpListener) -> (PacketReader<TcpStream>, PacketWriter<TcpStream>) {
    let (s, _) = server.accept().unwrap();
    (PacketReader::new(s.try_clone().unwrap()), PacketWriter::new(s))
}

/// Reads the handshake and login start; returns the player name.
fn read_login(r: &mut PacketReader<TcpStream>) -> String {
    let (id, body) = r.read().unwrap();
    let mut b = Buf(&body);
    assert_eq!((id, b.varint().unwrap()), (0x00, 47));
    assert_eq!(b.string(255).unwrap(), "127.0.0.1");
    b.u16().unwrap();
    assert_eq!(b.varint().unwrap(), 2, "next state is login");
    let (id, body) = r.read().unwrap();
    assert_eq!(id, 0x00);
    Buf(&body).string(16).unwrap()
}

fn varint(v: i32) -> Vec<u8> {
    let mut out = Vec::new();
    out.varint(v);
    out
}

#[test]
fn online_mode_login_encrypts_compresses_and_plays() {
    let (join_url, session_service) = mock_session_service();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap().to_string();
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).unwrap();
    let public_der = RsaPublicKey::from(&key).to_public_key_der().unwrap().as_bytes().to_vec();
    let der = public_der.clone();

    let server_thread = thread::spawn(move || {
        let (mut r, mut w) = accept(&server);
        assert_eq!(read_login(&mut r), "Tester");
        let token = [9u8, 8, 7, 6];
        let mut request = Vec::new();
        request.string("").varint(der.len() as i32).be(&der).varint(4).be(&token);
        w.write(0x01, &request).unwrap();

        let (id, body) = r.read().unwrap();
        assert_eq!(id, 0x01);
        let mut b = Buf(&body);
        let secret = key.decrypt(Pkcs1v15Encrypt, b.bytes(256).unwrap()).unwrap();
        assert_eq!(key.decrypt(Pkcs1v15Encrypt, b.bytes(256).unwrap()).unwrap(), token);
        let secret: [u8; 16] = secret.try_into().unwrap();
        r.enable_encryption(&secret);
        w.enable_encryption(&secret);

        w.write(0x03, &varint(64)).unwrap();
        r.set_compression(Some(64));
        w.set_compression(Some(64));
        let mut success = Vec::new();
        success.string("069a79f4-44e9-4726-a5be-fca90e38aaf5").string("Tester");
        w.write(0x02, &success).unwrap();

        // Play: a keep-alive the client must echo, then a large (compressed) chat.
        w.write(0x00, &varint(42)).unwrap();
        let (id, body) = r.read().unwrap();
        assert_eq!((id, Buf(&body).varint().unwrap()), (0x00, 42));
        let mut chat = Vec::new();
        chat.string(&format!(r#"{{"text":"{}"}}"#, "a".repeat(500))).be(&[0]);
        w.write(0x02, &chat).unwrap();
        let (id, body) = r.read().unwrap();
        assert_eq!((id, Buf(&body).string(100).unwrap()), (0x01, "hello".to_string()));
        secret
    });

    let profile = Profile {
        name: "Tester",
        session: Some(Session {
            access_token: "token-123",
            uuid: "069a79f444e94726a5befca90e38aaf5",
            join_url: &join_url,
        }),
    };
    let joined = join(&address, &profile).unwrap();
    assert_eq!(joined.name, "Tester");
    assert_eq!(joined.uuid, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
    let (mut r, mut w) = (joined.reader, joined.writer);

    let (id, body) = r.read().unwrap();
    let Clientbound::KeepAlive(n) = decode(id, &body, true).unwrap() else { panic!("expected keep-alive") };
    let (id, body) = send::keep_alive(n);
    w.write(id, &body).unwrap();
    let (id, body) = r.read().unwrap();
    let Clientbound::Chat { text, .. } = decode(id, &body, true).unwrap() else { panic!("expected chat") };
    assert_eq!(text, "a".repeat(500));
    let (id, body) = send::chat("hello");
    w.write(id, &body).unwrap();

    let secret = server_thread.join().unwrap();
    let joined: serde_json::Value = serde_json::from_str(&session_service.join().unwrap()).unwrap();
    assert_eq!(joined["accessToken"], "token-123");
    assert_eq!(joined["selectedProfile"], "069a79f444e94726a5befca90e38aaf5");
    assert_eq!(joined["serverId"], server_hash("", &secret, &public_der));
}

#[test]
fn offline_servers_need_no_account_and_online_ones_do() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap().to_string();
    let server_thread = thread::spawn(move || {
        // First client: offline mode, straight to success.
        let (mut r, mut w) = accept(&server);
        assert_eq!(read_login(&mut r), "Guest");
        let mut success = Vec::new();
        success.string("00000000-0000-3000-8000-000000000000").string("Guest");
        w.write(0x02, &success).unwrap();
        // Second client: the server asks for encryption.
        let (mut r, mut w) = accept(&server);
        read_login(&mut r);
        let mut request = Vec::new();
        request.string("").varint(3).be(&[1, 2, 3]).varint(1).be(&[0]);
        w.write(0x01, &request).unwrap();
        // Third client: kicked during login.
        let (mut r, mut w) = accept(&server);
        read_login(&mut r);
        let mut kick = Vec::new();
        kick.string(r#"{"text":"You are banned"}"#);
        w.write(0x00, &kick).unwrap();
    });
    let guest = Profile { name: "Guest", session: None };
    assert_eq!(join(&address, &guest).unwrap().name, "Guest");
    assert!(matches!(join(&address, &guest), Err(Error::NeedsAccount)));
    match join(&address, &guest) {
        Err(Error::Disconnected(reason)) => assert_eq!(reason, "You are banned"),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("joined a server that kicked us"),
    }
    server_thread.join().unwrap();
}

#[test]
fn status_ping_reports_version_and_players() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap().to_string();
    let server_thread = thread::spawn(move || {
        let (mut r, mut w) = accept(&server);
        let (_, hello) = r.read().unwrap();
        let mut b = Buf(&hello);
        b.varint().unwrap();
        b.string(255).unwrap();
        b.u16().unwrap();
        assert_eq!(b.varint().unwrap(), 1, "next state is status");
        assert_eq!(r.read().unwrap(), (0x00, Vec::new()));
        let json = r#"{"version":{"name":"Paper 1.20.4","protocol":765},"players":{"max":100,"online":7},"description":{"text":"§aA server"}}"#;
        let mut response = Vec::new();
        response.string(json);
        w.write(0x00, &response).unwrap();
    });
    let s = status(&address, std::time::Duration::from_secs(5)).unwrap();
    assert_eq!((s.version.as_str(), s.protocol, s.players), ("Paper 1.20.4", 765, (7, 100)));
    assert_eq!(s.description, "A server");
    server_thread.join().unwrap();
}
