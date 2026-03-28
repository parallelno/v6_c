// opt_small_inline_specialize.c - inline specialization
//
// Feature: inline small known functions and optimize parameter constants.
// Benefit: removes call overhead and enables further peephole optimization.
// Example:
//   int add1(int x) { return x + 1; }
//   y = add1(3); // inline to y=4

int y;

int add1(int x) {
    return x + 1;
}

void main(void) {
    y = add1(3);
}
