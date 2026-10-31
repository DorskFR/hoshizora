use tracing::instrument;

use crate::db::Star;

/// O(n²) over request data: compares every pair of stars.
#[instrument(skip(stars), fields(star_count = stars.len()))]
pub fn find_duplicate_stars(stars: &[Star]) -> Vec<(String, String)> {
    let mut dupes = Vec::new();
    for a in stars {
        for b in stars {
            if a.id < b.id
                && a.constellation == b.constellation
                && (a.distance_ly - b.distance_ly).abs() < 1.0
            {
                dupes.push((a.name.clone(), b.name.clone()));
            }
        }
    }
    dupes
}

#[instrument]
pub fn distance_modulus(distance_ly: f64) -> f64 {
    let parsecs = distance_ly / 3.2616;
    5.0 * parsecs.log10() - 5.0
}

/// Blocks the tokio worker thread while the lens "warms up".
#[instrument]
pub fn warm_lens() {
    std::thread::sleep(std::time::Duration::from_millis(40));
}

#[allow(dead_code)]
#[instrument]
pub fn legacy_magnitude_correction(magnitude: f64, airmass: f64) -> f64 {
    magnitude - 0.28 * (airmass - 1.0)
}
