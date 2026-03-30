/* Feature: Parameterized asm block with two int arguments. */
/* Benefit:  Verifies both args are placed correctly: first int in HL, */
/*           second int in DE, enabling register-to-register operations. */
/* Method:   add_and_store(int a, int b) uses asm(int a, int b) with */
/*           DAD D to add HL+DE, then SHLD to store the result. */
/* Expect:   Compiles without error. Output contains DAD D. */

int result;

void add_and_store(int a, int b) {
    asm(int a, int b) {
        DAD D
        SHLD _g_result
    };
}

void main(void) {
    add_and_store(10, 20);
}
