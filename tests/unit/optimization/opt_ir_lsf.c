/* opt_ir_lsf.c - IR load/store forwarding coverage */

int g0;
int g1;
int g2;

int touch(int v)
{
    g2 = v + 1;
    return g2;
}

int main(void)
{
    int pos;
    int neg;
    int safe;
    int *p;

    /* positive: g0=10; g1=g0=10; pos=10 */
    g0 = 10;
    g1 = g0;
    pos = g1;

    /* negative: g0=3; g1=4; neg=7 */
    g0 = 3;
    g1 = 4;
    neg = g0 + g1;

    /* safety: *p=20 → g0=20; safe=20+touch(5)=26; g1=g0=20; safe=46 */
    p = &g0;
    *p = 20;
    safe = g0;
    safe = safe + touch(5);
    g1 = g0;
    safe = safe + g1;

    if (pos != 10) return 1;
    if (neg != 7) return 2;
    if (safe != 46) return 3;
    return 0;
}
