/* Feature: Parameterized asm block with empty parameter list asm(){}. */
/* Benefit:  An empty param list tells the compiler this asm block */
/*           touches no registers, so no spill or clobber is needed. */
/* Method:   enable_interrupts() uses asm() { EI } which only sets the */
/*           interrupt flip-flop without affecting any registers. */
/* Expect:   Compiles without error. Output contains EI. */

void enable_interrupts() {
    asm() {
        EI
    };
}

void main(void) {
    enable_interrupts();
}
