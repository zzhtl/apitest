use std::io::Write;

use apitest_storage::BodyStore;

#[test]
fn atomically_commits_streamed_response_bodies() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let store = BodyStore::new(temp.path()).expect("body store should initialize");
    let mut sink = store.begin().expect("body sink should open");

    sink.write_all(b"hello ").expect("first chunk should write");
    sink.write_all(b"world").expect("second chunk should write");
    let body = sink.commit().expect("body should commit");

    assert_eq!(body.size, 11);
    assert_eq!(
        store.read_all(&body).expect("body should read"),
        b"hello world"
    );
    assert!(body.path.exists());
}

#[test]
fn dropping_an_uncommitted_sink_removes_the_temporary_file() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let store = BodyStore::new(temp.path()).expect("body store should initialize");
    let temporary_path = {
        let sink = store.begin().expect("body sink should open");
        sink.temporary_path().to_owned()
    };

    assert!(!temporary_path.exists());
}

#[test]
fn redacts_secrets_even_when_they_cross_stream_chunks() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let store = BodyStore::new(temp.path()).expect("body store should initialize");
    let mut sink = store
        .begin_redacted(["token-secret", "secret"])
        .expect("redacting body sink should open");

    sink.write_all(b"token=token-")
        .expect("first chunk should write");
    sink.write_all(b"secret; repeated=secret")
        .expect("second chunk should write");
    let body = sink.commit().expect("redacted body should commit");

    let stored = store.read_all(&body).expect("redacted body should read");
    assert_eq!(stored, b"token=[REDACTED]; repeated=[REDACTED]");
    assert!(!String::from_utf8_lossy(&stored).contains("token-secret"));
}

#[test]
fn flush_preserves_a_partial_secret_for_the_next_write() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let store = BodyStore::new(temp.path()).expect("body store should initialize");
    let mut sink = store
        .begin_redacted(["token-secret"])
        .expect("redacting body sink should open");

    sink.write_all(b"token=token-")
        .expect("secret prefix should write");
    sink.flush().expect("mid-stream flush should succeed");
    sink.write_all(b"secret")
        .expect("secret suffix should write");
    let body = sink.commit().expect("redacted body should commit");

    assert_eq!(
        store.read_all(&body).expect("redacted body should read"),
        b"token=[REDACTED]"
    );
}
