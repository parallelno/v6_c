/* Feature: Full-body asm function with two integer parameters. */
/* Benefit:  Write an entire function in assembly, bypassing the */
/*           code generator for maximum control. */
/* Method:   add(int a, int b) has a single asm block body. */
/*           Per calling convention a->HL, b->DE; DAD D adds them. */
/* Expect:   Compiles without error. Output contains DAD D and no */
/*           compiler-generated parameter prologue (SHLD _l_add_...). */

int result;

int add(int a, int b) {
    asm {
        DAD D
    }
}

void main(void) {
    result = add(10, 20);
}
