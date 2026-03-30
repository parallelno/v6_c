/* Feature: Raw asm block with internal labels and conditional jumps. */
/* Benefit:  Proves that user-defined labels inside a raw asm block are */
/*           emitted verbatim and can serve as branch targets. */
/* Method:   count_to_ten() contains a _ctt_loop: label with JNZ forming */
/*           a loop that increments HL from 0 to 10. */
/* Expect:   Compiles without error. Output contains _ctt_loop: label. */

int result;

int count_to_ten() {
    int counter;
    counter = 0;
    asm {
        LXI H, 0
_ctt_loop:
        INX H
        MOV A,L
        CPI 10
        JNZ _ctt_loop
        SHLD _l_count_to_ten_counter
    }
    return counter;
}

void main(void) {
    result = count_to_ten();
}
