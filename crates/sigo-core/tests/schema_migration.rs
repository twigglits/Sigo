use sigo_core::TurnRecord;

#[test]
fn v1_record_loads_into_v2_with_default_none_cache_fields() {
    let raw = include_str!("fixtures/v1_turn_record.json");
    let r: TurnRecord =
        serde_json::from_str(raw).expect("v1 record should still deserialise under v2 schema");

    let ec = r
        .english_control_run
        .as_ref()
        .expect("the fixture is known to be a full-control-mode record");

    assert!(
        ec.cache_read_tokens_reported.is_none(),
        "v1 records have no cache_read field; deserialiser must default to None"
    );
    assert!(
        ec.cache_write_tokens_reported.is_none(),
        "v1 records have no cache_write field; deserialiser must default to None"
    );
}
