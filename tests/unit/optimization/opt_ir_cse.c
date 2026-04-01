/* opt_ir_cse.c - IR common sub-expression elimination coverage */

int a;
int b;
int guard;

int inc(int x)
{
    return x + 1;
}

int main(void)
{
    int pos;
    int neg;
    int safe;

    a = 11;
    b = 5;

    /* positive: (11+5)+(11+5) = 16+16 = 32 */
    pos = (a + b) + (a + b);

    /* negative: (11+5)+(11-5) = 16+6 = 22 */
    neg = (a + b) + (a - b);

    /* safety: (11+5)+inc(1) = 16+2 = 18 */
    guard = inc(1);
    safe = (a + b) + guard;

    if (pos != 32) return 1;
    if (neg != 22) return 2;
    if (safe != 18) return 3;
    return 0;
}
