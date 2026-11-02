//! `castle64::graph_crowd` on the PC (issue #138): the `graph crowd`
//! self-test step's script, whose hash every target (ares, Azahar, the
//! browsers) must reproduce, and its cost on this machine.

use castle64::graph_crowd;
use trino_platform_pc::PcPlatform;
use trino_platform_pc::SimProfile;

#[test]
fn the_graph_crowd_plays_the_pinned_script() {
    let mut platform = PcPlatform::new(&SimProfile::N64.caps());
    let c = graph_crowd::measure(&mut platform).unwrap();
    println!("graph crowd: {c:?}");
    let h = c.variants[0].hash;
    assert_eq!(
        h,
        graph_crowd::CROWD_HASH,
        "the crowd's playback moved: set CROWD_HASH to {h:#010x} if on purpose"
    );
    // The same updates give the same playback, and the crowd instance is a
    // fraction of the full one.
    let again = graph_crowd::measure(&mut platform).unwrap();
    assert_eq!(again.variants.map(|v| v.hash), c.variants.map(|v| v.hash));
    assert!(
        c.variants[2].instance_bytes * 3 < c.variants[0].instance_bytes,
        "{c:?}"
    );
}
