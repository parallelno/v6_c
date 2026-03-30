/* Feature: Raw asm block accessing global variables via _g_ labels. */
/* Benefit:  Allows hand-written assembly to directly manipulate globals */
/*           using the compiler naming convention _g_<var>. */
/* Method:   swap_globals() uses asm{} with LHLD/XCHG/SHLD to swap */
/*           _g_global_a and _g_global_b in-place. */
/* Expect:   Compiles without error. Output references both _g_global_a */
/*           and _g_global_b. */

int global_a;
int global_b;

void swap_globals() {
    asm {
        LHLD _g_global_a
        XCHG
        LHLD _g_global_b
        SHLD _g_global_a
        XCHG
        SHLD _g_global_b
    }
}

void main(void) {
    global_a = 100;
    global_b = 200;
    swap_globals();
}
