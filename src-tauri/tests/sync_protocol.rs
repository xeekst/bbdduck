//! Integration test for the sync protocol: starts a real Node A server on
//! 127.0.0.1, lists shares/files and streams a file back, verifying the wire
//! protocol used by the download workers.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bbdduck_lib::sync::client::{list_remote_files, list_shares};
use bbdduck_lib::sync::protocol::{
    connect_with_timeout, mtime_secs, read_msg, safe_join, write_msg, ClientMsg, FileEntry,
    ServerMsg, PROTOCOL_VERSION,
};
use bbdduck_lib::sync::server::ServerHandle;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bbdduck-test-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_tree(root: &Path) -> Vec<(String, u64)> {
    fs::create_dir_all(root.join("sub/dir")).unwrap();
    let mut out = Vec::new();
    for (name, content) in [
        ("a.txt", b"hello world".to_vec()),
        ("sub/b.bin", vec![7u8; 100_000]),
        ("sub/dir/c.log", b"line1\nline2\n".to_vec()),
    ] {
        let p = root.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, &content).unwrap();
        out.push((name.to_string(), content.len() as u64));
    }
    out
}

#[test]
fn server_lists_and_streams_files() {
    let src = temp_dir("src");
    let files = write_tree(&src);

    let server = ServerHandle::new();
    let addr = server
        .start("127.0.0.1".into(), 0, vec![src.to_string_lossy().to_string()])
        .expect("start server");
    let (ip, port) = addr.rsplit_once(':').map(|(i, p)| {
        (i.to_string(), p.parse::<u16>().expect("port"))
    }).unwrap();

    // 1. list shares
    let shares = list_shares(&ip, port).expect("list_shares");
    assert_eq!(shares, vec![src.to_string_lossy().to_string()]);

    // 2. list remote files (streamed)
    let mut entries: Vec<FileEntry> = Vec::new();
    let (total, total_bytes) = list_remote_files(&ip, port, &shares[0], |e| {
        entries.push(e.clone());
        true
    })
    .expect("list_remote_files");

    assert_eq!(total, files.len() as u64);
    let expected_bytes: u64 = files.iter().map(|(_, s)| s).sum();
    assert_eq!(total_bytes, expected_bytes);

    // every file must be present in the listing
    for (name, size) in &files {
        let found = entries
            .iter()
            .find(|e| e.path == *name && !e.is_dir)
            .expect("file in listing");
        assert_eq!(found.size, *size);
    }

    // 3. stream a file over the protocol (what download workers do)
    let mut stream = connect_with_timeout(&ip, port, 5).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    write_msg(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
    let _ack = read_msg::<_, ServerMsg>(&mut stream).unwrap().unwrap();
    let target = files[1].0.clone(); // sub/b.bin (100_000 bytes)
    write_msg(
        &mut stream,
        &ClientMsg::FetchFile {
            share: shares[0].clone(),
            path: target.clone(),
        },
    )
    .unwrap();
    let meta = match read_msg::<_, ServerMsg>(&mut stream).unwrap().unwrap() {
        ServerMsg::FileMeta { size, mtime } => (size, mtime),
        other => panic!("expected FileMeta, got {other:?}"),
    };
    assert_eq!(meta.0, files[1].1);
    let local_md = fs::metadata(src.join(&target)).unwrap();
    assert_eq!(meta.1, mtime_secs(&local_md));

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    assert_eq!(buf.len() as u64, files[1].1);
    assert_eq!(buf, fs::read(src.join(&target)).unwrap());

    // 4. path traversal is rejected
    let mut stream2 = connect_with_timeout(&ip, port, 5).expect("connect");
    write_msg(&mut stream2, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
    let _ = read_msg::<_, ServerMsg>(&mut stream2).unwrap().unwrap();
    write_msg(
        &mut stream2,
        &ClientMsg::FetchFile {
            share: shares[0].clone(),
            path: "../secret.txt".into(),
        },
    )
    .unwrap();
    match read_msg::<_, ServerMsg>(&mut stream2).unwrap().unwrap() {
        ServerMsg::Error { .. } => {}
        other => panic!("expected Error for traversal, got {other:?}"),
    }

    // 5. safe_join unit checks
    assert!(safe_join(&src, "../x").is_none());
    assert!(safe_join(&src, "a/b").is_some());
    assert!(safe_join(&src, "/etc/passwd").is_none());

    server.stop();
    let _ = fs::remove_dir_all(&src);
}

#[test]
fn incremental_mtime_rule() {
    // sanity: a fresh local file reports a comparable mtime
    let d = temp_dir("mtime");
    let p = d.join("x");
    fs::write(&p, b"data").unwrap();
    let md = fs::metadata(&p).unwrap();
    let secs = mtime_secs(&md);
    assert!(secs > 1_600_000_000, "mtime should be a unix timestamp, got {secs}");
    let _ = fs::remove_dir_all(&d);
}
