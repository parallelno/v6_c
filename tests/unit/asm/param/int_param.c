/* Feature: Parameterized asm block with a single int argument. */
/* Benefit:  Verifies the compiler places an int arg into the HL register */
/*           pair per calling convention before entering the asm block. */
/* Method:   store_int(int x) uses asm(int x) { SHLD _g_result } to */
/*           store HL directly to the global result variable. */
/* Expect:   Compiles without error. Output contains SHLD _g_result. */

int result;

void store_int(int x) {
    asm(int x) {
        SHLD _g_result
    };
}

int main(void) {
    store_int(100);
    if (result != 100) return 1;
    return 0;
}
