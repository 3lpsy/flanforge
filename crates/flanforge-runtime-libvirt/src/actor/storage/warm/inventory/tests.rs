use super::{parse_domain_disks, reconcile_views, xml_backing_path};

const VOLUME_WITH_BACKING: &str = r#"<volume type="file"><name>root-1.qcow2</name>
  <target><path>/pool/root-1.qcow2</path><format type="qcow2"/></target>
  <backingStore><path>/pool/warm-abc.qcow2</path><format type="qcow2"/></backingStore>
</volume>"#;

const VOLUME_WITHOUT_BACKING: &str = r#"<volume type="file"><name>warm-abc.qcow2</name>
  <target><path>/pool/warm-abc.qcow2</path><format type="qcow2"/></target>
</volume>"#;

#[test]
fn a_volume_declaring_a_backing_store_reports_its_path() {
    assert_eq!(
        xml_backing_path(VOLUME_WITH_BACKING),
        Ok(Some("/pool/warm-abc.qcow2".to_owned()))
    );
    assert_eq!(xml_backing_path(VOLUME_WITHOUT_BACKING), Ok(None));
}

/// The whole point of the two-instrument proof: neither reading alone may
/// clear a candidate, and disagreement is fail-closed.
#[test]
fn a_missing_backing_store_element_is_inconclusive_rather_than_absence() {
    let path = || Some("/pool/warm-abc.qcow2".to_owned());
    assert_eq!(reconcile_views(None, None), Ok(None));
    assert_eq!(reconcile_views(path(), path()), Ok(path()));
    // libvirt could not probe the file, but the file itself names a parent.
    assert!(reconcile_views(path(), None).is_err());
    // libvirt names a parent the file does not.
    assert!(reconcile_views(None, path()).is_err());
    assert!(
        reconcile_views(path(), Some("/pool/other.qcow2".to_owned())).is_err(),
        "two different parents must never be read as one"
    );
}

/// A pool directory reached through a symlink spells the same file two ways,
/// which is why the comparison is normalized on both sides.
#[test]
fn two_spellings_of_one_backing_path_agree() {
    assert_eq!(
        reconcile_views(
            Some("/pool/warm-abc.qcow2".to_owned()),
            Some("/pool/./sub/../warm-abc.qcow2".to_owned()),
        ),
        Ok(Some("/pool/warm-abc.qcow2".to_owned()))
    );
}

#[test]
fn an_oversized_volume_document_is_refused_rather_than_parsed() {
    let oversized = format!(
        "<volume><name>{}</name></volume>",
        "a".repeat(1_024 * 1_024)
    );
    assert!(xml_backing_path(&oversized).is_err());
}

/// A domain naming the candidate as a disk is a reference just as much as one
/// naming it as a backing file.
#[test]
fn a_domain_reports_pool_addressed_and_file_addressed_disks() {
    let xml = r#"<domain type="kvm"><name>g</name><devices>
      <disk type="volume" device="disk"><source pool="flanforge" volume="root-1.qcow2"/></disk>
      <disk type="file" device="disk"><source file="/elsewhere/other.qcow2"/>
        <backingStore type="file"><source file="/pool/warm-abc.qcow2"/></backingStore></disk>
    </devices></domain>"#;
    let disks = parse_domain_disks(xml).unwrap_or_else(|error| unreachable!("parse: {error}"));
    assert_eq!(
        disks.paths,
        vec![
            "/elsewhere/other.qcow2".to_owned(),
            "/pool/warm-abc.qcow2".to_owned()
        ]
    );
    assert_eq!(
        disks.volumes,
        vec![("flanforge".to_owned(), "root-1.qcow2".to_owned())]
    );
}

/// A live domain's XML carries its whole chain, and every level of it is a
/// reference. An inactive domain's does not, which is why each disk is also
/// resolved through libvirt and asked what it is backed by.
#[test]
fn a_domain_reports_every_level_of_a_chain_it_declares() {
    let xml = r#"<domain type="kvm"><name>g</name><devices>
      <disk type="file" device="disk"><source file="/pool/root-1.qcow2"/>
        <backingStore type="file"><source file="/pool/warm-abc.qcow2"/>
          <backingStore type="file"><source file="/pool/base.qcow2"/></backingStore>
        </backingStore></disk>
      <disk type="file" device="disk"><source file="/elsewhere/inactive.qcow2"/></disk>
    </devices></domain>"#;
    let disks = parse_domain_disks(xml).unwrap_or_else(|error| unreachable!("parse: {error}"));
    assert_eq!(
        disks.paths,
        vec![
            "/pool/root-1.qcow2".to_owned(),
            "/pool/warm-abc.qcow2".to_owned(),
            "/pool/base.qcow2".to_owned(),
            "/elsewhere/inactive.qcow2".to_owned(),
        ]
    );
    assert!(disks.volumes.is_empty());
}

#[test]
fn an_oversized_domain_document_is_refused_rather_than_parsed() {
    let oversized = format!(
        "<domain><name>{}</name></domain>",
        "a".repeat(1_024 * 1_024)
    );
    assert!(parse_domain_disks(&oversized).is_err());
}
