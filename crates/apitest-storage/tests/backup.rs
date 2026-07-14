use apitest_core::Project;
use apitest_storage::{BackupManager, Database};

#[test]
fn creates_consistent_backups_and_enforces_retention() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let database_path = temp.path().join("apitest.db");
    let backups_path = temp.path().join("backups");
    let database = Database::open(&database_path).expect("database should open");
    database
        .save_project(&Project::new("Before backup"))
        .expect("project should save");
    let manager = BackupManager::new(&backups_path, 2).expect("manager should initialize");

    for _ in 0..3 {
        manager
            .snapshot(&database)
            .expect("snapshot should succeed");
    }

    let backups = manager.list().expect("backups should list");
    assert_eq!(backups.len(), 2);
    let restored = Database::open(&backups[0]).expect("backup should be a valid database");
    assert_eq!(
        restored
            .list_projects()
            .expect("backup should contain data")[0]
            .name,
        "Before backup"
    );
}
