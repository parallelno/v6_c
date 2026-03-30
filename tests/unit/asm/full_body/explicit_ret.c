/* Feature: Full-body asm function with an explicit RET instruction. */
/* Benefit:  Confirms the compiler detects RET at the end of an asm body */
/*           and does not emit a duplicate RET. */
/* Method:   add_explicit_ret ends with DAD D / RET. Called twice to */
/*           prevent inlining. */
/* Expect:   Compiles without error. Output contains DAD D and exactly */
/*           one RET (or HLT when inlined). */

int result;

int add_explicit_ret(int a, int b) {
    asm {
        DAD D
        RET
    }
}

void main(void) {
    result = add_explicit_ret(10, 20);
    result = add_explicit_ret(30, 40);
}
