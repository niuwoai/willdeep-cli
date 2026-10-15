// Keep the first value for each key, ignoring later duplicates.
function reviewed(input) {
  const output = Object.create(null);
  for (const entry of input.entries) {
    output[entry.key] = entry.value;
  }
  return output;
}
