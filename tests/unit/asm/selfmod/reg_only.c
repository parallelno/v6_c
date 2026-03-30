/* Feature: Register-only equate to suppress parameter allocation. */
/* Benefit:  Setting _l_<func>_<param> = 0 tells the frame allocator */
/*           the parameter lives only in a register, saving stack space. */
/* Method:   just_return(int x) sets _l_just_return_x = 0 and relies on */
/*           x already being in HL per calling convention. */
/* Expect:   Compiles without error. */

int result;

int just_return(int x) {
    asm {
_l_just_return_x = 0
        ; x stays in HL, return it
    }
}

void main(void) {
    result = just_return(7);
}
