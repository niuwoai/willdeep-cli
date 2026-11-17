/// Amounts are cents; the caller supplies values whose subtotal fits in u64.
pub fn quote_cents(unit_cents: u64, quantity: u32, discount_bps: u16) -> u64 {
    let subtotal = unit_cents * u64::from(quantity);
    subtotal - subtotal * u64::from(discount_bps.min(10_000)) / 10_000
}
