from app.photometry import apparent_magnitude, compute_all


def test_vega_is_reference_magnitude():
    assert abs(apparent_magnitude(1.0, 25.0) - (5 * 0.8845 - 5)) < 0.01


def test_compute_all_keeps_ids():
    out = compute_all([{"id": 7, "flux": 2.0, "distance_ly": 10.0}])
    assert [m["id"] for m in out] == [7]
