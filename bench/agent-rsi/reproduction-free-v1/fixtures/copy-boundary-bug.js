// Copy each input value exactly once, in order, with no extra values.
function reviewed(input) {
  const output = [];
  for (let index = 0; index <= input.values.length; index++) {
    output.push(input.values[index]);
  }
  return output;
}
