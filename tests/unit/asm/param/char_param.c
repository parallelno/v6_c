/* Feature: Parameterized asm block with a single char argument. */
/* Benefit:  Verifies the compiler places a char arg into the A register */
/*           per calling convention before entering the asm block. */
/* Method:   out_byte(char val) uses asm(char val) { OUT 42 } to output */
/*           the byte value to port 42. */
/* Expect:   Compiles without error. Output contains OUT 42. */

void out_byte(char val) {
    asm(char val) {
        OUT 42
    };
}

int main(void) {
    out_byte(0xFF);
    return 0;
}
