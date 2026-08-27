use apitest_core::{
    ApiDefinition, Environment, HttpMethod, HttpSpec, Project, ProtocolSpec, Variable,
};
use apitest_storage::Database;

#[test]
fn persists_projects_definitions_and_environments() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("Petstore");
    let definition = ApiDefinition::new(
        "List pets",
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "{{base_url}}/pets")),
    );
    let environment = Environment::new("Development")
        .with_variable(Variable::plain("base_url", "http://localhost:8080"));

    database
        .save_project(&project)
        .expect("project should save");
    database
        .save_definition(project.id, &definition)
        .expect("definition should save");
    database
        .save_environment(project.id, &environment)
        .expect("environment should save");

    assert_eq!(
        database.list_projects().expect("projects should load"),
        vec![project.clone()]
    );
    assert_eq!(
        database
            .list_definitions(project.id)
            .expect("definitions should load"),
        vec![definition]
    );
    assert_eq!(
        database
            .list_environments(project.id)
            .expect("environments should load"),
        vec![environment]
    );
}

#[test]
fn searches_definition_names_without_loading_every_document() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("Search");
    database
        .save_project(&project)
        .expect("project should save");

    for name in ["Create user", "Delete user", "List orders"] {
        let definition = ApiDefinition::new(
            name,
            ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.test")),
        );
        database
            .save_definition(project.id, &definition)
            .expect("definition should save");
    }

    let matches = database
        .search_definitions(project.id, "user", 10)
        .expect("search should succeed");

    assert_eq!(matches.len(), 2);
    assert!(matches.iter().all(|item| item.name.contains("user")));
}

#[test]
fn deletes_definitions_from_storage_and_search() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("Delete definition");
    let definition = ApiDefinition::new(
        "Disposable request",
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.test")),
    );
    database
        .save_project(&project)
        .expect("project should save");
    database
        .save_definition(project.id, &definition)
        .expect("definition should save");

    assert!(
        database
            .delete_definition(project.id, definition.id)
            .expect("definition should delete")
    );
    assert!(
        database
            .list_definitions(project.id)
            .expect("definitions should load")
            .is_empty()
    );
    assert!(
        database
            .search_definitions(project.id, "Disposable", 10)
            .expect("search should succeed")
            .is_empty()
    );
}

#[test]
fn deletes_environments_without_affecting_the_project() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("Delete environment");
    let environment = Environment::new("Temporary");
    database
        .save_project(&project)
        .expect("project should save");
    database
        .save_environment(project.id, &environment)
        .expect("environment should save");

    assert!(
        database
            .delete_environment(project.id, environment.id)
            .expect("environment should delete")
    );
    assert!(
        database
            .list_environments(project.id)
            .expect("environments should load")
            .is_empty()
    );
    assert_eq!(
        database.list_projects().expect("projects should load"),
        vec![project]
    );
}

#[test]
fn reopening_an_older_database_rebuilds_the_search_index_keyed_by_rowid() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let path = temp.path().join("apitest-v2.sqlite3");
    let project = Project::new("Petstore");
    let definition = ApiDefinition::new(
        "List users",
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "/users")),
    );
    {
        let database = Database::open(&path).expect("database should open");
        database
            .save_project(&project)
            .expect("project should save");
        database
            .save_definition(project.id, &definition)
            .expect("definition should save");
    }
    {
        // Fake the v2 on-disk state: version 2, FTS rows not aligned with the
        // definitions rowids.
        let connection = rusqlite::Connection::open(&path).expect("raw connection should open");
        connection
            .execute(
                "UPDATE metadata SET value = '2' WHERE key = 'schema_version'",
                [],
            )
            .expect("version should downgrade");
        connection
            .execute("DELETE FROM definitions_fts", [])
            .expect("index should clear");
        connection
            .execute(
                "INSERT INTO definitions_fts(rowid, id, project_id, name, description)
                 VALUES(999, ?1, ?2, ?3, '')",
                rusqlite::params![
                    definition.id.to_string(),
                    project.id.to_string(),
                    definition.name
                ],
            )
            .expect("stale index row should insert");
    }

    let database = Database::open(&path).expect("database should reopen");
    assert_eq!(
        database.schema_version().expect("version should read"),
        3,
        "reopening must migrate to the rowid-keyed index"
    );
    let hits = database
        .search_definitions(project.id, "users", 10)
        .expect("search should work after migration");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, definition.id);

    // The rowid keying must survive a save + delete roundtrip.
    database
        .save_definition(project.id, &definition)
        .expect("definition should re-save");
    assert!(
        database
            .delete_definition(project.id, definition.id)
            .expect("definition should delete")
    );
    assert!(
        database
            .search_definitions(project.id, "users", 10)
            .expect("search should still work")
            .is_empty()
    );
}
