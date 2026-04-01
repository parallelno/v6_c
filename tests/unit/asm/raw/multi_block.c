/* Feature: Multiple raw asm blocks within a single function. */
/* Benefit:  Confirms the compiler correctly handles several separate */
/*           asm{} blocks in sequence, each acting as a memory barrier. */
/* Method:   multi_block() has two consecutive asm{} blocks: the first */
/*           stores 1 to _g_result, the second increments it to 2. */
/* Expect:   Compiles without error. */

int result;

void multi_block() {
    asm {
        LXI H, 1
        SHLD _g_result
    }
    asm {
        LHLD _g_result
        INX H
        SHLD _g_result
    }
}

int main(void) {
    multi_block();
    if (result != 2) return 1;
    return 0;
}
