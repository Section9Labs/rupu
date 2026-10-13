//! `rupu_cp::host::skill_ship`: the per-host ship memo and bundle packing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rupu_cp::host::connector::HostConnectorError;
use rupu_cp::host::skill_ship::{manifest_of, ShipMemo, SkillBundle, Transfer};
use rupu_skills::ship::{ShipError, MAX_SHIPPED_SKILL_BYTES};
use rupu_skills::{load_skill, Layer};

fn write_skill(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nDo {name}.\n"),
    )
    .unwrap();
    root
}

fn bundle_of(dir: &std::path::Path, names: &[&str]) -> SkillBundle {
    SkillBundle::new(
        names
            .iter()
            .map(|n| Arc::new(load_skill(&write_skill(dir, n), Layer::Global).unwrap()))
            .collect(),
    )
}

#[tokio::test]
async fn concurrent_ships_of_one_set_transfer_once() {
    let tmp = tempfile::tempdir().unwrap();
    let packed = bundle_of(tmp.path(), &["pdf", "sql"]).pack().unwrap();
    let memo = Arc::new(ShipMemo::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let (memo, calls, packed) = (memo.clone(), calls.clone(), packed.clone());
            tokio::spawn(async move {
                memo.ship(&packed, |pending| async move {
                    assert_eq!(pending.len(), 2);
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(Transfer::Done)
                })
                .await
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    memo.ship(&packed, |_| async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Transfer::Done)
    })
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a later ship transfers nothing"
    );
}

/// A host that can't take shipped skills is asked once per connector: every
/// later ship, concurrent or not, answers the remembered downgrade.
#[tokio::test]
async fn a_downgrade_is_remembered() {
    let tmp = tempfile::tempdir().unwrap();
    let packed = bundle_of(tmp.path(), &["pdf"]).pack().unwrap();
    let memo = Arc::new(ShipMemo::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let ship = |memo: Arc<ShipMemo>, calls: Arc<AtomicUsize>, packed: Vec<_>| async move {
        memo.ship(&packed, |_| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(Transfer::Downgrade("host h: too old".into()))
        })
        .await
    };
    let tasks: Vec<_> = (0..20)
        .map(|_| tokio::spawn(ship(memo.clone(), calls.clone(), packed.clone())))
        .collect();
    for t in tasks {
        assert_eq!(
            t.await.unwrap().unwrap(),
            Transfer::Downgrade("host h: too old".into())
        );
    }
    assert_eq!(
        ship(memo.clone(), calls.clone(), packed.clone())
            .await
            .unwrap(),
        Transfer::Downgrade("host h: too old".into())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failed_transfer_confirms_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let packed = bundle_of(tmp.path(), &["pdf"]).pack().unwrap();
    let memo = ShipMemo::default();
    let err = memo
        .ship(&packed, |_| async {
            Err(HostConnectorError::Unreachable("dropped".into()))
        })
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Unreachable(_)));
    let calls = AtomicUsize::new(0);
    memo.ship(&packed, |pending| {
        assert_eq!(pending.len(), 1);
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(Transfer::Done) }
    })
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the next ship transfers again"
    );
}

#[tokio::test]
async fn only_the_unconfirmed_skills_are_transferred() {
    let tmp = tempfile::tempdir().unwrap();
    let both = bundle_of(tmp.path(), &["pdf", "sql"]).pack().unwrap();
    let memo = ShipMemo::default();
    memo.ship(&both[..1], |_| async { Ok(Transfer::Done) })
        .await
        .unwrap();
    memo.ship(&both, |pending| {
        assert_eq!(
            pending.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["sql"]
        );
        async { Ok(Transfer::Done) }
    })
    .await
    .unwrap();
}

/// Each skill fits the per-skill cap; together they don't fit the bundle's.
/// Sparse files, so nothing is read: the cap is checked from a stat pass.
#[test]
fn a_bundle_over_the_total_cap_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let names = ["a1", "a2", "a3", "a4", "a5"];
    for n in names {
        let root = write_skill(tmp.path(), n);
        let f = std::fs::File::create(root.join("blob.bin")).unwrap();
        f.set_len(MAX_SHIPPED_SKILL_BYTES - 1024).unwrap();
    }
    let bundle = SkillBundle::new(
        names
            .iter()
            .map(|n| Arc::new(load_skill(&tmp.path().join(n), Layer::Global).unwrap()))
            .collect(),
    );
    assert!(matches!(
        bundle.pack(),
        Err(ShipError::BundleTooLarge { .. })
    ));
}

#[test]
fn an_over_cap_skill_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let root = write_skill(tmp.path(), "big");
    let f = std::fs::File::create(root.join("blob.bin")).unwrap();
    f.set_len(MAX_SHIPPED_SKILL_BYTES + 1).unwrap();
    let bundle = SkillBundle::new(vec![Arc::new(load_skill(&root, Layer::Global).unwrap())]);
    assert!(matches!(
        bundle.pack(),
        Err(ShipError::TooLarge { name, .. }) if name == "big"
    ));
}

#[test]
fn an_edited_skill_repacks_and_the_manifest_lists_every_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = bundle_of(tmp.path(), &["pdf", "sql"]);
    let first = bundle.pack().unwrap();
    let again = bundle.pack().unwrap();
    assert!(
        Arc::ptr_eq(&first[0], &again[0]),
        "an unchanged skill is cached"
    );
    let m = manifest_of(&first);
    assert_eq!(m.version, 1);
    assert_eq!(
        m.skills.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["pdf", "sql"]
    );

    std::fs::write(tmp.path().join("pdf").join("notes.md"), "new file").unwrap();
    let edited = bundle.pack().unwrap();
    assert_ne!(edited[0].hash, first[0].hash, "an edited skill repacks");
    assert!(Arc::ptr_eq(&first[1], &edited[1]));
}
