/* opt_regalloc_pressure.c - register pressure/rematerialization coverage */

int main(void)
{
    int a;
    int b;
    int c;
    int d;
    int e;
    int f;
    int g;
    int h;
    int pos;
    int neg;
    int safe;

    a = 1;
    b = 2;
    c = 3;
    d = 4;
    e = 5;
    f = 6;
    g = 7;
    h = 8;

    /* positive: 1+2+3+4+5+6+7+8 = 36 */
    pos = a + b + c + d;
    pos = pos + e + f + g + h;

    /* negative: 1+100+100+100 = 301 */
    neg = a;
    neg = neg + 100;
    neg = neg + 100;
    neg = neg + 100;

    /* safety: 42*4 = 168 */
    safe = 42 + 42 + 42 + 42;

    if (pos != 36) return 1;
    if (neg != 301) return 2;
    if (safe != 168) return 3;
    return 0;
}
