//! End-to-end install against a mock distribution server: manifest, version
//! metadata, libraries, natives, client jar and asset objects, then the same
//! install again with the server gone (offline launch).

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rustcraft_assets::download::sha1_hex;
use rustcraft_assets::launch::Endpoints;
use rustcraft_assets::version::Os;
use rustcraft_assets::{Downloader, Installer, Platform, http_client_builder};
use rustcraft_core::DataDir;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        out.start_file(*name, opts).unwrap();
        out.write_all(data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

async fn serve(server: &MockServer, at: &str, body: Vec<u8>) {
    Mock::given(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn installs_then_relaunches_offline() {
    // A bare (non-pooled) server, so dropping it really closes the port.
    let server = MockServer::builder().start().await;
    let uri = server.uri();

    let jar = b"client jar".to_vec();
    let lib = b"gson".to_vec();
    let natives = zip(&[
        ("liblwjgl.so", b"elf"),
        ("META-INF/MANIFEST.MF", b"skip me"),
        ("../../escape.so", b"evil"),
    ]);
    let sound = b"ogg data".to_vec();
    let index = json!({ "objects": {
        "minecraft/sounds/a.ogg": { "hash": sha1_hex(&sound), "size": sound.len() },
        "minecraft/sounds/same-content.ogg": { "hash": sha1_hex(&sound), "size": sound.len() }
    }})
    .to_string()
    .into_bytes();
    let version = json!({
        "id": "1.8.9",
        "type": "release",
        "mainClass": "net.minecraft.client.main.Main",
        "minecraftArguments": "--username ${auth_player_name}",
        "assetIndex": { "id": "1.8", "sha1": sha1_hex(&index), "size": index.len(), "url": format!("{uri}/index.json") },
        "downloads": { "client": { "sha1": sha1_hex(&jar), "size": jar.len(), "url": format!("{uri}/client.jar") } },
        "libraries": [
            { "name": "com.google.code.gson:gson:2.2.4",
              "downloads": { "artifact": { "path": "com/google/code/gson/gson/2.2.4/gson-2.2.4.jar",
                "sha1": sha1_hex(&lib), "size": lib.len(), "url": format!("{uri}/gson.jar") } } },
            { "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
              "natives": { "linux": "natives-linux" },
              "extract": { "exclude": ["META-INF/"] },
              "downloads": { "classifiers": { "natives-linux": {
                "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-linux.jar",
                "sha1": sha1_hex(&natives), "size": natives.len(), "url": format!("{uri}/natives.jar") } } } }
        ]
    })
    .to_string()
    .into_bytes();
    let manifest = json!({
        "latest": { "release": "1.8.9", "snapshot": "1.8.9" },
        "versions": [ { "id": "1.8.9", "type": "release", "url": format!("{uri}/1.8.9.json"),
            "releaseTime": "2015-12-03T09:24:39+00:00", "sha1": sha1_hex(&version) } ]
    })
    .to_string()
    .into_bytes();

    serve(&server, "/manifest.json", manifest).await;
    serve(&server, "/1.8.9.json", version).await;
    serve(&server, "/index.json", index).await;
    serve(&server, "/client.jar", jar.clone()).await;
    serve(&server, "/gson.jar", lib).await;
    serve(&server, "/natives.jar", natives).await;
    serve(
        &server,
        &format!("/res/{}/{}", &sha1_hex(&sound)[..2], sha1_hex(&sound)),
        sound.clone(),
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let dirs = DataDir::at(tmp.path().join("data"));
    let mut installer = Installer::new(
        dirs.clone(),
        Downloader::new(http_client_builder().build().unwrap())
            .with_backoff(std::time::Duration::from_millis(1)),
    );
    installer.platform = Platform {
        os: Os::Linux,
        arch: "x86_64",
    };
    installer.endpoints = Endpoints {
        manifest: format!("{uri}/manifest.json"),
        resources: format!("{uri}/res"),
    };

    let manifest = installer.manifest().await.unwrap();
    let version = installer.resolve("1.8.9", Some(&manifest)).await.unwrap();
    let progress = Arc::default();
    let installed = installer
        .install(&version, &progress, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(dirs.root().join("versions/1.8.9/1.8.9.jar")).unwrap(),
        jar
    );
    assert_eq!(installed.classpath.len(), 2);
    assert_eq!(installed.assets_index, "1.8");
    assert_eq!(
        std::fs::read(installed.natives_dir.join("liblwjgl.so")).unwrap(),
        b"elf"
    );
    assert!(!installed.natives_dir.join("META-INF").exists());
    assert!(!tmp.path().join("escape.so").exists() && !dirs.root().join("escape.so").exists());
    let object = dirs
        .assets()
        .join("objects")
        .join(&sha1_hex(&sound)[..2])
        .join(sha1_hex(&sound));
    assert_eq!(std::fs::read(object).unwrap(), sound);
    assert_eq!(
        progress.files_done.load(Ordering::Relaxed),
        progress.files_total.load(Ordering::Relaxed)
    );
    assert_eq!(progress.fraction(), 1.0);

    // Offline: no server. Cached metadata and verified files must suffice.
    drop(server);
    let manifest = installer.manifest().await.unwrap();
    let version = installer.resolve("1.8.9", Some(&manifest)).await.unwrap();
    installer
        .install(&version, &Arc::default(), &CancellationToken::new())
        .await
        .unwrap();

    // A corrupted library is detected and, with no network, reported.
    std::fs::write(
        dirs.libraries()
            .join("com/google/code/gson/gson/2.2.4/gson-2.2.4.jar"),
        b"tampered",
    )
    .unwrap();
    let err = installer
        .install(&version, &Arc::default(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("gson.jar"), "{err}");
}
