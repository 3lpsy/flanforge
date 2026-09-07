use super::is_directory_pool_xml;

fn pool(kind: &str) -> String {
    format!(
        r#"<pool type="{kind}"><name>flanforge</name><target><path>/pool</path></target></pool>"#
    )
}

/// Warm images are admitted on a directory pool and nowhere else: anywhere a
/// volume key and a backing path are different strings, the retirement proof
/// can never reconcile them.
#[test]
fn only_a_directory_pool_admits_warm_images() {
    assert_eq!(is_directory_pool_xml(&pool("dir")), Ok(true));
    for kind in ["logical", "rbd", "gluster", "netfs", "zfs"] {
        assert_eq!(is_directory_pool_xml(&pool(kind)), Ok(false), "{kind}");
    }
}

/// Nothing readable means nothing provable, so a document that cannot be
/// parsed refuses rather than answering either way.
#[test]
fn an_unreadable_pool_document_refuses_rather_than_answering() {
    assert!(is_directory_pool_xml("<pool type=\"dir\"").is_err());
    assert!(is_directory_pool_xml("").is_err());
    let oversized = format!(
        "<pool type=\"dir\"><name>{}</name></pool>",
        "a".repeat(1_024 * 1_024)
    );
    assert!(is_directory_pool_xml(&oversized).is_err());
    // A pool element that is not the document root is still the pool.
    assert_eq!(
        is_directory_pool_xml(r#"<pools><pool type="dir"><name>flanforge</name></pool></pools>"#),
        Ok(true)
    );
    // A document with no pool element at all names no type.
    assert_eq!(
        is_directory_pool_xml("<capabilities><host/></capabilities>"),
        Ok(false)
    );
}
