/* opt_ir_inline_specialize.c - IR inlining/specialization coverage */

int addk(int x, int k)
{
    return x + k;
}

int big_func(int x)
{
    int i;
    int r;
    r = x;
    for (i = 0; i < 6; i = i + 1) {
        r = r + i;
    }
    return r;
}

int main(void)
{
    int pos;
    int neg;
    int safe;
    int t;

    /* positive: addk(10,2)+addk(11,2) = 12+13 = 25 */
    pos = addk(10, 2);
    pos = pos + addk(11, 2);

    /* negative: addk(7,3) = 10 */
    t = 3;
    neg = addk(7, t);

    /* safety: big_func(5): r=5+0+1+2+3+4+5 = 20 */
    safe = big_func(5);

    if (pos != 25) return 1;
    if (neg != 10) return 2;
    if (safe != 20) return 3;
    return 0;
}
