// opt_small_empty.c - empty function baseline
//
// Feature: constant folding of pure arithmetic expressions.
// Benefit: reduces instruction count and removes runtime computation.
//
// This test verifies that no code will be produced for an empty main.

int main(void) {
	return 0;
}
