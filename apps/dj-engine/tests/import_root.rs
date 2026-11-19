// The folder an import writes into decides where the DJ finds the track afterwards.
//
// An empty root used to be taken as-is: the copies landed in the engine's working directory and the index kept
// their relative paths, so the collection list — which asks about absolute folders on a disk — showed nothing
// over a folder full of tracks. These two are the rules that keep that from happening again.

#[test]
fn an_import_without_a_folder_lands_in_the_collection() {
    let root = dj_engine::import_root("   ");
    assert!(
        root.is_absolute(),
        "the collection folder has to be a real place, not {}",
        root.display()
    );
    assert!(
        root.ends_with("hauler-dj/collection"),
        "the fallback is the engine's own collection folder, got {}",
        root.display()
    );
}

#[test]
fn a_relative_import_folder_is_made_absolute() {
    let root = dj_engine::import_root("some/music");
    assert!(
        root.is_absolute(),
        "a relative folder would put the copies next to the engine, not in the library: {}",
        root.display()
    );
    assert!(
        root.ends_with("some/music"),
        "the name the DJ gave is kept, got {}",
        root.display()
    );
}
