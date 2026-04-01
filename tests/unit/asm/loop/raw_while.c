/* Feature: Raw asm block inside a while loop. */
/* Benefit:  Verifies that the compiler correctly spills and restores */
/*           registers around a raw asm block on every loop iteration. */
/* Method:   raw_in_loop() uses asm{} inside while(i<5) to increment */
/*           _g_result via LHLD/INX H/SHLD each iteration. */
/* Expect:   Compiles without error. */

int result;

void raw_in_loop() {
    int i;
    result = 0;
    i = 0;
    while (i < 5) {
        asm {
            LHLD _g_result
            INX H
            SHLD _g_result
        }
        i = i + 1;
    }
}

int main(void) {
    raw_in_loop();
    if (result != 5) return 1;
    return 0;
}
