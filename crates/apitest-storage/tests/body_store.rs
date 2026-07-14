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
