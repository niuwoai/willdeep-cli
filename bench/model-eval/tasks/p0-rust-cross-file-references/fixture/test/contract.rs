use reference_shop::{checkout, invoice, pricing, shipping, AUDIT_EVENT};

#[test]
fn references_keep_the_pricing_contract_for_varied_inputs() {
    let mut seed = 0x5a17_u64;
    for _ in 0..192 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let unit = (seed >> 9) % 50_000;
        let quantity = (seed % 31) as u32;
        let discount = ((seed >> 21) % 12_001) as u16;
        let subtotal = unit * u64::from(quantity);
        let expected = subtotal - subtotal * u64::from(discount.min(10_000)) / 10_000;
        assert_eq!(pricing::quote_cents(unit, quantity, discount), expected);
        assert_eq!(invoice::line_total(unit, quantity, discount), expected);
        for remote in [false, true] {
            let postage = if remote { 1_250 } else { 450 };
            assert_eq!(checkout::total(unit, quantity, discount, remote), expected + postage);
        }
    }
}

#[test]
fn boundary_rounding_and_unrelated_names_are_preserved() {
    for (unit, quantity, discount, expected) in [
        (0, 4, 250, 0),
        (101, 0, 250, 0),
        (101, 3, 333, 293),
        (999, 2, 10_000, 0),
        (999, 2, 12_000, 0),
    ] {
        assert_eq!(pricing::quote_cents(unit, quantity, discount), expected);
    }
    assert_eq!(shipping::quote(false), 450);
    assert_eq!(shipping::quote(true), 1_250);
    assert_eq!(AUDIT_EVENT, "quote");
}
