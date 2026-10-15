// Zero is valid; only null or undefined uses fallback.
function reviewed(input) {
  return input.count || input.fallback;
}
