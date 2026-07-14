use apitest_core::SecretRef;
use apitest_storage::{MemorySecretStore, SecretStore};

#[test]
fn secret_store_never_requires_plaintext_project_data() {
    let store = MemorySecretStore::default();
    let reference = SecretRef::new("keyring://project/token");

    store
        .set(&reference, "super-secret")
        .expect("secret should save");
    assert_eq!(
        store
            .get(&reference)
            .expect("secret should load")
            .as_deref(),
        Some("super-secret")
    );

    store.delete(&reference).expect("secret should delete");
    assert_eq!(store.get(&reference).expect("lookup should succeed"), None);
}
