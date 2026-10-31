use super::*;

/// A 16 by 16 picture that is red everywhere.
const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0xf3, 0xff,
    0x61, 0x00, 0x00, 0x00, 0x19, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x9f, 0x12, 0xcc, 0x30, 0x6a, 0xc0, 0xa8, 0x01, 0xa3, 0x06, 0x0c, 0x17, 0x03, 0x00, 0x30, 0xc4,
    0xfe, 0x10, 0x8a, 0xc6, 0x4c, 0xe5, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

#[test]
fn a_portrait_fills_the_row_less_its_gap_at_every_density() {
    assert_eq!(portrait_size(Density::Dense), 16.0);
    assert_eq!(portrait_size(Density::Normal), 20.0);
    assert_eq!(portrait_size(Density::Sparse), 24.0);
}

#[test]
fn a_kept_portrait_draws_with_the_name_and_a_missing_one_stays_blank() {
    let env = env();
    let dir = env.data.join("images");
    fs::create_dir_all(&dir).expect("the image folder");
    fs::write(images::cache_path(&dir, Picture::Character(MAIN)), RED_PNG)
        .expect("the kept portrait");
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    // The app's own fetcher would reach the image server; keep the missing picture offline.
    // `init` replaces the state, so this also forgets what the app loaded before.
    images::init(&harness.ctx, &dir, Arc::new(NoImages));
    assert!(!images::is_loaded(&harness.ctx, Picture::Character(MAIN)));
    until(&mut harness, "Main's portrait", |h| {
        images::is_loaded(&h.ctx, Picture::Character(MAIN))
    });
    assert!(images::is_loaded(&harness.ctx, Picture::Character(MAIN)));
    assert!(!images::is_loaded(&harness.ctx, Picture::Character(ALT)));
    assert!(shown(&harness, "Main"));
    assert!(shown(&harness, "Alt"));
}

#[test]
fn the_characters_page_matches_its_snapshot() {
    let env = env();
    // A fixed folder, so the path the panel shows is the same on every run.
    let jeve = std::env::temp_dir().join("mop-characters-snapshot");
    drop(fs::remove_dir_all(&jeve));
    fs::create_dir_all(&jeve).expect("the folder");
    fill_jeve(&jeve);
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.clone());
    eve.warning = Some(KEYCHAIN.to_owned());
    let mut harness = open(&env, &eve);
    until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
    frames(&mut harness);
    crate::ui::no_images(&mut harness);
    harness.snapshot("characters");
    fs::remove_dir_all(&jeve).expect("the folder goes");
}
