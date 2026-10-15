function reviewed(input) {
  return input.entries.reduce((output, entry) => {
    if (!Object.hasOwn(output, entry.key)) output[entry.key] = entry.value;
    return output;
  }, Object.create(null));
}
