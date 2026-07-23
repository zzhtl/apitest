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
