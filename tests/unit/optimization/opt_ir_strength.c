/* opt_ir_strength.c - IR strength reduction coverage */

int x;
unsigned int ux;

int main(void)
{
    int pos;
    int neg;
    int safe;

    x = 9;

    /* positive: 9*8+9*4+9*2 = 72+36+18 = 126 */
    pos = (x * 8) + (x * 4) + (x * 2);

    /* negative: 9*7 = 63 */
    neg = x * 7;

    /* safety: 123/8=15 remainder 3; safe=18 */
    ux = 123;
    safe = (int)(ux / 8);
    safe = safe + (int)(ux % 8);

    if (pos != 126) return 1;
    if (neg != 63) return 2;
    if (safe != 18) return 3;
    return 0;
}
