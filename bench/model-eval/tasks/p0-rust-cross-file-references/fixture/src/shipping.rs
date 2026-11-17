pub fn quote(remote: bool) -> u64 {
    if remote { 1_250 } else { 450 }
}
