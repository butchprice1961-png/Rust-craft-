//! A tiny offline-mode protocol 47 server for trying RustCraft's
//! multiplayer client: a flat creative world with another player and a
//! wandering pig, chat echo, and block breaking and placing. It is a test tool, not a game server: no physics,
//! no mobs, no saving, no authentication.
//!
//! Usage: cargo run -p rustcraft-protocol --example flat_server [port]

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glam::IVec3;

use rustcraft_protocol::codec::{Buf, Put};
use rustcraft_protocol::conn::{PacketReader, PacketWriter};
use rustcraft_protocol::{PROTOCOL_VERSION, Result};

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(25565);
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("flat_server listening on 127.0.0.1:{port}");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            if let Err(e) = serve(stream) {
                println!("connection closed: {e}");
            }
        });
    }
    Ok(())
}

fn body(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut v = Vec::new();
    f(&mut v);
    v
}

/// One column: bedrock, two stone, dirt, grass; full sky light above.
fn flat_column(out: &mut Vec<u8>) {
    let surface = |y: usize| match y {
        0 => 7u16,
        1 | 2 => 1,
        3 => 3,
        4 => 2,
        _ => 0,
    };
    for i in 0..4096 {
        out.extend_from_slice(&(surface(i >> 8) << 4).to_le_bytes());
    }
    out.extend_from_slice(&[0; 2048]);
    for i in (0..4096).step_by(2) {
        let sky = |i: usize| if (i >> 8) > 4 { 15u8 } else { 0 };
        out.push(sky(i) | (sky(i + 1) << 4));
    }
    out.extend_from_slice(&[1; 256]);
}

fn serve(stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut r = PacketReader::new(stream.try_clone()?);
    let w = Arc::new(Mutex::new(PacketWriter::new(stream)));
    let send = |id: i32, b: Vec<u8>| w.lock().unwrap().write(id, &b);

    let (_, hello) = r.read()?;
    let mut b = Buf(&hello);
    let (protocol, _, _, next) = (b.varint()?, b.string(255)?, b.u16()?, b.varint()?);
    if next == 1 {
        r.read()?;
        let status = r#"{"version":{"name":"1.8.9","protocol":47},"players":{"max":8,"online":0},"description":{"text":"RustCraft test server"}}"#;
        send(
            0x00,
            body(|v| {
                v.string(status);
            }),
        )?;
        let (_, ping) = r.read()?;
        return send(0x01, ping);
    }
    if protocol != PROTOCOL_VERSION {
        return send(
            0x00,
            body(|v| {
                v.string(r#"{"text":"This test server speaks Minecraft 1.8.x"}"#);
            }),
        );
    }
    let (_, start) = r.read()?;
    let name = Buf(&start).string(16)?;
    println!("joined: {name}");

    send(
        0x03,
        body(|v| {
            v.varint(256);
        }),
    )?;
    r.set_compression(Some(256));
    w.lock().unwrap().set_compression(Some(256));
    send(
        0x02,
        body(|v| {
            v.string("00000000-0000-3000-8000-000000000001").string(&name);
        }),
    )?;
    // Join game: creative, overworld, normal difficulty, flat.
    send(
        0x01,
        body(|v| {
            v.be(&1i32.to_be_bytes()).be(&[1, 0, 2, 8]).string("flat").be(&[0]);
        }),
    )?;
    send(
        0x05,
        body(|v| {
            v.position(IVec3::new(0, 5, 0));
        }),
    )?;
    send(
        0x39,
        body(|v| {
            v.be(&[0x0D]).be(&0.05f32.to_be_bytes()).be(&0.1f32.to_be_bytes());
        }),
    )?;
    let columns: Vec<(i32, i32)> = (-5..=5).flat_map(|x| (-5..=5).map(move |z| (x, z))).collect();
    send(
        0x26,
        body(|v| {
            v.be(&[1]).varint(columns.len() as i32);
            for (x, z) in &columns {
                v.be(&x.to_be_bytes()).be(&z.to_be_bytes()).be(&1u16.to_be_bytes());
            }
            for _ in &columns {
                flat_column(v);
            }
        }),
    )?;
    send(
        0x03,
        body(|v| {
            v.be(&0i64.to_be_bytes()).be(&6000i64.to_be_bytes());
        }),
    )?;
    let teleport = body(|v| {
        for c in [0.5f64, 5.0, 0.5] {
            v.be(&c.to_be_bytes());
        }
        v.be(&0f32.to_be_bytes()).be(&30f32.to_be_bytes()).be(&[0]);
    });
    send(0x08, teleport)?;
    let chat = |text: &str| {
        body(|v| {
            v.string(&serde_json::json!({ "text": text }).to_string()).be(&[0]);
        })
    };
    send(0x02, chat(&format!("Welcome, {name}. This is RustCraft's test server.")))?;

    // Company: a player called Steve (with a tab-list entry) and a pig.
    let steve: u128 = 0x0000_0000_0000_3000_8000_0000_0000_0002;
    send(
        0x38,
        body(|v| {
            v.varint(0).varint(1).be(&steve.to_be_bytes()).string("Steve").varint(0).varint(1).varint(20).be(&[0]);
        }),
    )?;
    let fixed = |v: &mut Vec<u8>, x: f64, y: f64, z: f64| {
        for c in [x, y, z] {
            v.be(&((c * 32.0) as i32).to_be_bytes());
        }
    };
    send(
        0x0C,
        body(|v| {
            v.varint(2).be(&steve.to_be_bytes());
            fixed(v, 3.5, 5.0, 4.5);
            v.be(&[0, 0]).be(&0i16.to_be_bytes()).be(&[0x7f]);
        }),
    )?;
    send(
        0x0F,
        body(|v| {
            v.varint(3).be(&[90]);
            fixed(v, -2.5, 5.0, 4.5);
            v.be(&[0, 0, 0]).be(&[0; 6]).be(&[0x7f]);
        }),
    )?;

    // Keep-alives every two seconds; the pig wanders back and forth.
    let keep_alive = w.clone();
    std::thread::spawn(move || {
        for n in 1.. {
            std::thread::sleep(Duration::from_millis(100));
            let step: i8 = if (n / 20) % 2 == 0 { 4 } else { -4 };
            let mut moved = Vec::new();
            moved.varint(3).be(&[step as u8, 0, 0]).be(&[1]);
            let mut writer = keep_alive.lock().unwrap();
            let alive = if n % 20 == 0 {
                let mut p = Vec::new();
                p.varint(n);
                writer.write(0x00, &p)
            } else {
                Ok(())
            };
            if alive.and_then(|()| writer.write(0x15, &moved)).is_err() {
                return;
            }
        }
    });

    let mut held = None;
    loop {
        let (id, packet) = r.read()?;
        let mut b = Buf(&packet);
        match id {
            0x01 => {
                let message = b.string(100)?;
                println!("chat: {message}");
                send(0x02, chat(&format!("<{name}> {message}")))?;
            }
            0x07 => {
                let (status, pos) = (b.u8()?, b.position()?);
                if status == 0 {
                    println!("dig {} {} {}", pos.x, pos.y, pos.z);
                    send(
                        0x23,
                        body(|v| {
                            v.position(pos).varint(0);
                        }),
                    )?;
                }
            }
            0x08 => {
                let (pos, face) = (b.position()?, b.u8()?);
                let item = b.slot()?.or(held);
                let offsets = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
                if let (Some(offset), Some(item)) = (offsets.get(face as usize), item)
                    && (1..256).contains(&item.id)
                {
                    let target = pos + IVec3::from_array(*offset);
                    println!("place {} {} {} {}", target.x, target.y, target.z, item.id);
                    let state = ((item.id as i32) << 4) | (item.damage as i32 & 15);
                    send(
                        0x23,
                        body(|v| {
                            v.position(target).varint(state);
                        }),
                    )?;
                }
            }
            0x10 => {
                let slot = b.i16()?;
                held = b.slot()?;
                println!("creative slot {slot}: {held:?}");
            }
            _ => {}
        }
    }
}
