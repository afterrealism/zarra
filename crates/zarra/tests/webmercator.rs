#![cfg(feature = "local")]

use zarra::create::webmercator::{
    level_shapes, native_maxzoom, pick_dst_nodata, tile_range_for, webmerc_res, webmerc_tilespan,
    WORLD,
};
use zarra::{DType, Nodata};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn near(actual: f64, expected: f64, epsilon: f64) -> bool {
    (actual - expected).abs() <= epsilon
}

#[test]
fn native_zoom_matches_exact_webmercator_resolutions() -> TestResult {
    for z in [0, 3, 8, 12, 18, 22] {
        assert_eq!(native_maxzoom(webmerc_res(z)?)?, z);
    }
    let finer = native_maxzoom(webmerc_res(10)? * 0.9)?;
    assert!(finer == 10 || finer == 11);
    assert_eq!(native_maxzoom(1e12)?, 0);
    Ok(())
}

#[test]
fn zoom_and_native_pixel_inputs_are_checked() {
    assert!(webmerc_res(23).is_err());
    assert!(webmerc_tilespan(23).is_err());
    assert!(native_maxzoom(0.0).is_err());
    assert!(native_maxzoom(-1.0).is_err());
    assert!(native_maxzoom(f64::NAN).is_err());
    assert!(native_maxzoom(f64::INFINITY).is_err());
}

#[test]
fn dtype_mapping_matches_the_profile() {
    assert_eq!(DType::from_gdal("Float32"), Some(DType::Float32));
    assert_eq!(DType::from_gdal("Byte"), Some(DType::Uint8));
    assert_eq!(DType::from_gdal("UInt16"), Some(DType::Uint16));
    assert_eq!(DType::from_gdal("Int16"), Some(DType::Int16));
    assert_eq!(DType::from_gdal("CFloat32"), None);
}

#[test]
fn whole_world_and_northwest_ranges_are_tile_aligned() -> TestResult {
    let world = tile_range_for(0, -WORLD, -WORLD, WORLD, WORLD)?;
    assert_eq!((world.tx0, world.tx1, world.ty0, world.ty1), (0, 0, 0, 0));
    assert!(near(world.te_minx, -WORLD, 1e-3));
    assert!(near(world.te_maxx, WORLD, 1e-3));
    assert!(near(world.te_miny, -WORLD, 1e-3));
    assert!(near(world.te_maxy, WORLD, 1e-3));
    assert_eq!((world.ncols(), world.nrows()), (1, 1));

    let northwest = tile_range_for(1, -WORLD, 0.0, 0.0, WORLD)?;
    assert_eq!(
        (northwest.tx0, northwest.tx1, northwest.ty0, northwest.ty1),
        (0, 0, 0, 0)
    );
    assert!(near(northwest.te_minx, -WORLD, 1e-3));
    assert!(near(northwest.te_maxx, 0.0, 1e-3));
    assert!(near(northwest.te_miny, 0.0, 1e-3));
    assert!(near(northwest.te_maxy, WORLD, 1e-3));
    Ok(())
}

#[test]
fn centered_area_touches_both_sides_of_tile_boundaries() -> TestResult {
    let span = webmerc_tilespan(2)?;
    let distance = span * 0.25;
    let range = tile_range_for(2, -distance, -distance, distance, distance)?;
    assert_eq!((range.tx0, range.tx1), (1, 2));
    assert_eq!((range.ty0, range.ty1), (1, 2));
    assert!(near(
        range.te_maxx - range.te_minx,
        f64::from(range.ncols()) * span,
        1e-3
    ));
    assert!(near(
        range.te_maxy - range.te_miny,
        f64::from(range.nrows()) * span,
        1e-3
    ));
    Ok(())
}

#[test]
fn aligned_coverage_holds_at_representative_levels() -> TestResult {
    for z in [5, 10, 14] {
        let span = webmerc_tilespan(z)?;
        let range = tile_range_for(z, -1_234_567.0, 890_123.0, 2_345_678.0, 4_567_890.0)?;
        let remainder = (range.te_maxx - range.te_minx) % span;
        assert!(
            near(remainder, 0.0, span * 1e-6) || near(remainder, span, span * 1e-6),
            "level {z} width was not tile aligned"
        );
        assert!(range.te_minx <= -1_234_567.0 + 1e-3);
        assert!(range.te_maxx >= 2_345_678.0 - 1e-3);
    }
    Ok(())
}

#[test]
fn tile_range_rejects_invalid_geometry() {
    assert!(tile_range_for(23, 0.0, 0.0, 1.0, 1.0).is_err());
    assert!(tile_range_for(0, f64::NAN, 0.0, 1.0, 1.0).is_err());
    assert!(tile_range_for(0, 2.0, 0.0, 1.0, 1.0).is_err());
    assert!(tile_range_for(0, 0.0, 2.0, 1.0, 1.0).is_err());
}

#[test]
fn nodata_selection_covers_integer_float_and_multiband_cases() -> TestResult {
    let cases = [
        (DType::Uint8, vec![[10.0, 245.0]], Some(0.0)),
        (DType::Uint8, vec![[0.0, 245.0]], Some(246.0)),
        (DType::Uint8, vec![[0.0, 255.0]], None),
        (DType::Int16, vec![[-5.0, 300.0]], Some(-6.0)),
        (DType::Uint8, vec![[0.0, 100.0], [200.0, 255.0]], None),
        (DType::Uint8, vec![[5.0, 100.0], [200.0, 250.0]], Some(0.0)),
        (DType::Uint16, vec![[1.0, 100.0]], Some(0.0)),
        (DType::Uint32, vec![[1.0, 100.0]], Some(0.0)),
        (DType::Int32, vec![[-5.0, 300.0]], Some(-6.0)),
        (DType::Int8, vec![[-128.0, 127.0]], None),
        (DType::Uint8, vec![], Some(0.0)),
    ];
    for (dtype, ranges, expected) in cases {
        let actual = pick_dst_nodata(dtype, &ranges)?;
        assert_eq!(actual, expected.map(Nodata::Number), "{dtype:?} {ranges:?}");
    }
    assert_eq!(
        pick_dst_nodata(DType::Float32, &[[-100.0, 8848.0]])?,
        Some(Nodata::NaN)
    );
    assert_eq!(pick_dst_nodata(DType::Float64, &[])?, Some(Nodata::NaN));
    Ok(())
}

#[test]
fn one_band_drops_band_axis_but_multiband_keeps_it() -> TestResult {
    assert_eq!(level_shapes(1, 3, 2)?, ("512,768".into(), "256,256".into()));
    assert_eq!(
        level_shapes(3, 3, 2)?,
        ("3,512,768".into(), "3,256,256".into())
    );
    assert_eq!(
        level_shapes(4, 9, 1)?,
        ("4,256,2048".into(), "4,256,256".into())
    );
    Ok(())
}

#[test]
fn shape_arithmetic_is_checked() {
    assert!(level_shapes(0, 1, 1).is_err());
    assert!(level_shapes(1, 0, 1).is_err());
    assert!(level_shapes(1, 1, 0).is_err());
    assert!(level_shapes(1, u32::MAX, 1).is_err());
}
