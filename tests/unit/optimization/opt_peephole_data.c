/* opt_peephole_data.c - peephole data-movement coverage */

int g;

int main(void)
{
    int x;
    int y;
    int pos;
    int neg;
    int safe;

    /* positive: x=3; g=3; g=g (nop); pos=3 */
    x = 3;
    g = x;
    g = g;
    pos = g;

    /* negative: y=5; neg=3+5=8 */
    y = 5;
    neg = x + y;

    /* safety: g=8; safe=8 */
    g = neg;
    safe = g;

    if (pos != 3) return 1;
    if (neg != 8) return 2;
    if (safe != 8) return 3;
    return 0;
}
