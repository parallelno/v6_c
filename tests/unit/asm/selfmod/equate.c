/* Feature: Self-modifying code using the = * + 1 equate pattern. */
/* Benefit:  The * + 1 idiom lets an equate label point at the operand */
/*           byte of the following instruction, enabling runtime patching. */
/* Method:   save_and_restore(int x) uses _sar_save = * + 1 before SHLD */
/*           and _sar_restore = * + 1 before LXI H to create a */
/*           save/restore pair that patches its own operands. */
/* Expect:   Compiles without error. Output contains "= * + 1". */

int save_and_restore(int x) {
    asm {
_sar_save = * + 1
        SHLD 0
        ; HL now saved in the LXI operand below
        LXI H, 0
        ; ... do some work that trashes HL ...
        ; restore HL
_sar_restore = * + 1
        LXI H, 0
    }
}

int main(void) {
    save_and_restore(42);
    return 0;
}
