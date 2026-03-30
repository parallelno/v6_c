/* Feature: Parameterized asm block storing a char via STA. */
/* Benefit:  Confirms that a char argument is available in A inside the */
/*           asm block and can be written to memory with STA. */
/* Method:   toggle_bit(char mask) uses asm(char mask) { STA _g_byte_result } */
/*           to store the byte directly to a global variable. */
/* Expect:   Compiles without error. Output contains STA. */

char byte_result;

void toggle_bit(char mask) {
    asm(char mask) {
        ; A has the mask value
        STA _g_byte_result
    };
}

void main(void) {
    toggle_bit(0x55);
}
