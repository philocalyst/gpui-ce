//! Opt-in, ignored microbenchmarks for Scene recording, finishing, and replay.
//!
//! Run with `cargo test -p gpui-ce --no-default-features --lib scene_workload::scene_record_replay_microbench -- --ignored --nocapture`.

use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, DevicePixels, Point,
    PolychromeSprite, Quad, ScaledPixels, Scene, ShaderBool, Size, TileId,
};
use std::time::Instant;

const ITEMS: usize = 10_000;
const FINISH_RUNS: usize = 500;
const FRAME_RUNS: usize = 50;

fn bounds() -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(0.0),
            y: ScaledPixels(0.0),
        },
        size: Size {
            width: ScaledPixels(100.0),
            height: ScaledPixels(100.0),
        },
    }
}

fn quad() -> Quad {
    let bounds = bounds();
    Quad {
        bounds,
        content_mask: ContentMask {
            bounds,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn polychrome_sprite(tile_id: u32) -> PolychromeSprite {
    let bounds = bounds();
    PolychromeSprite {
        order: 0,
        grayscale: ShaderBool::Disabled,
        opacity: 1.0,
        corner_smoothing: 0.0,
        bounds,
        content_mask: ContentMask {
            bounds,
            ..Default::default()
        },
        corner_radii: Default::default(),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Polychrome,
            },
            tile_id: TileId(tile_id),
            padding: 0,
            bounds: Bounds::<DevicePixels>::default(),
        },
    }
}

fn quad_scene() -> Scene {
    let mut scene = Scene::default();
    for _ in 0..ITEMS {
        scene.insert_primitive(quad());
    }
    scene
}

fn set_quad_orders(scene: &mut Scene, mut order: impl FnMut(usize) -> u32) {
    for (index, quad) in scene.quads.iter_mut().enumerate() {
        quad.order = order(index);
    }
}

fn sprite_scene() -> Scene {
    let mut scene = Scene::default();
    for index in 0..ITEMS {
        scene.insert_primitive(polychrome_sprite(index as u32));
    }
    scene
}

fn set_reverse_sprite_keys(scene: &mut Scene) {
    for (index, sprite) in scene.polychrome_sprites.iter_mut().enumerate() {
        sprite.order = 1;
        sprite.tile.tile_id = TileId((ITEMS - index) as u32);
    }
}

fn record_quads(scene: &mut Scene, order: impl FnMut(usize) -> u32) {
    for _ in 0..ITEMS {
        scene.insert_primitive(quad());
    }
    set_quad_orders(scene, order);
}

fn record_reverse_sprites(scene: &mut Scene) {
    for index in 0..ITEMS {
        scene.insert_primitive(polychrome_sprite(index as u32));
    }
    set_reverse_sprite_keys(scene);
}

fn warm_scene(record: impl FnOnce(&mut Scene)) -> Scene {
    let mut scene = Scene::default();
    record(&mut scene);
    scene.finish();
    scene.clear();
    scene
}

fn finish_time(mut scene: Scene, mut mutate: impl FnMut(&mut Scene)) -> u128 {
    scene.finish();
    let start = Instant::now();
    for _ in 0..FINISH_RUNS {
        mutate(&mut scene);
        scene.finish();
        std::hint::black_box(scene.render_commands());
    }
    start.elapsed().as_micros()
}

fn recording_time(mut scene: Scene, mut record: impl FnMut(&mut Scene)) -> u128 {
    let start = Instant::now();
    for run in 0..FRAME_RUNS {
        if run > 0 {
            scene.clear();
        }
        record(&mut scene);
        scene.finish();
        std::hint::black_box(scene.render_commands());
    }
    start.elapsed().as_micros()
}

fn replay_time(source: &Scene) -> u128 {
    let range = 0..source.len();
    let mut replay = Scene::default();
    replay.replay(range.clone(), source);
    replay.clear();

    let start = Instant::now();
    for _ in 0..FRAME_RUNS {
        replay.clear();
        replay.replay(range.clone(), source);
        std::hint::black_box(replay.len());
    }
    start.elapsed().as_micros()
}

#[test]
#[ignore = "manual scene microbenchmark; run with --ignored --nocapture"]
fn scene_record_replay_microbench() {
    let sorted_finish = finish_time(quad_scene(), |_| {});
    let reverse_finish = finish_time(quad_scene(), |scene| {
        set_quad_orders(scene, |index| (ITEMS - index) as u32)
    });
    let reverse_equal_finish = finish_time(quad_scene(), |scene| {
        set_quad_orders(scene, |index| ((ITEMS + 1 - index) / 2) as u32)
    });
    let one_swap_finish = finish_time(quad_scene(), |scene| {
        set_quad_orders(scene, |index| index as u32);
        scene.quads.swap(0, 1);
    });
    let mostly_sorted_finish = finish_time(quad_scene(), |scene| {
        set_quad_orders(scene, |index| {
            if index.is_multiple_of(1000) {
                (index + 1) as u32
            } else if index % 1000 == 1 {
                (index - 1) as u32
            } else {
                index as u32
            }
        });
    });
    let random_finish = finish_time(quad_scene(), |scene| {
        set_quad_orders(scene, |index| ((index * 7919) % ITEMS) as u32)
    });
    let reverse_sprite_finish = finish_time(sprite_scene(), set_reverse_sprite_keys);

    println!(
        "finish us / 500x10k: sorted={sorted_finish}, reverse={reverse_finish}, reverse_equal_runs={reverse_equal_finish}, one_swap={one_swap_finish}, mostly_sorted={mostly_sorted_finish}, random={random_finish}, reverse_sprite_ids={reverse_sprite_finish}"
    );

    let sorted_record = recording_time(
        warm_scene(|scene| record_quads(scene, |index| index as u32)),
        |scene| record_quads(scene, |index| index as u32),
    );
    let reverse_record = recording_time(
        warm_scene(|scene| record_quads(scene, |index| (ITEMS - index) as u32)),
        |scene| record_quads(scene, |index| (ITEMS - index) as u32),
    );
    let reverse_sprites_record =
        recording_time(warm_scene(record_reverse_sprites), record_reverse_sprites);
    println!(
        "record+finish us / 50x10k: sorted_quads={sorted_record}, reverse_quads={reverse_record}, reverse_sprite_ids={reverse_sprites_record}"
    );

    let mut replay_source = quad_scene();
    set_quad_orders(&mut replay_source, |index| (ITEMS - index) as u32);
    replay_source.finish();
    let replay = replay_time(&replay_source);
    println!("replay us / 50x10k reversed quads: {replay}");
}
