//! Fixture tests for the secdef DTOs. Note the deliberate conid type split:
//! secdef/search sends conid as a string; secdef/info sends it as an integer.
use oath_adapter_ibkr::cpapi::{SecdefInfo, SecdefSearchEntry, decode};

#[test]
fn secdef_search_conid_is_a_string() {
    let entries: Vec<SecdefSearchEntry> =
        decode(include_bytes!("fixtures/cpapi/secdef_search.json")).expect("search decodes");
    let e = entries.first().expect("one entry");
    assert_eq!(e.conid, "265598");
    assert_eq!(e.sections.len(), 2);
}

#[test]
fn secdef_info_conid_is_an_int() {
    let infos: Vec<SecdefInfo> =
        decode(include_bytes!("fixtures/cpapi/secdef_info.json")).expect("info decodes");
    let i = infos.first().expect("one info");
    assert_eq!(i.conid, 265_598);
    assert_eq!(i.sec_type.as_deref(), Some("STK"));
}
