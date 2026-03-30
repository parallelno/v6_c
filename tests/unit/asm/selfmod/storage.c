/* Feature: Inline data reservation using .STORAGE directive. */
/* Benefit:  Allows asm code to reserve scratch memory inline without */
/*           needing a separate data section or global variable. */
/* Method:   round_trip(int val) uses SHLD/LHLD with a _rt_temp: label */
/*           followed by .STORAGE 2 to save and reload HL. */
/* Expect:   Compiles without error. Output contains the _rt_temp: label. */

int result;

int round_trip(int val) {
    asm {
        SHLD _rt_temp
        LXI H, 0
        LHLD _rt_temp
        RET
_rt_temp:
        .STORAGE 2
    }
}

void main(void) {
    result = round_trip(123);
}
