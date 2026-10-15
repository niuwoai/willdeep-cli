function reviewed(input) {
  return input.count === null || input.count === undefined ? input.fallback : input.count;
}
