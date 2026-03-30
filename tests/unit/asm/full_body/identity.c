/* Feature: Full-body asm function with an empty asm body. */
/* Benefit:  Confirms the compiler emits no extra code around an asm */
/*           block that intentionally does nothing (arg already in HL). */
/* Method:   identity(int x) has an asm block containing only a comment. */
/*           The first int arg is already in HL per calling convention. */
/* Expect:   Compiles without error. The asm region is present in output. */

int result;

int identity(int x) {
    asm {
        ; x is in HL, nothing to do
    }
}

void main(void) {
    result = identity(42);
}
