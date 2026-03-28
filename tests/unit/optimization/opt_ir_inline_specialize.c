/* opt_ir_inline_specialize.c - IR inlining/specialization coverage */

int status;

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

void main(void)
{
    int pos;
    int neg;
    int safe;
    int t;

    /* positive: same constant argument at multiple call sites */
    pos = addk(10, 2);
    pos = pos + addk(11, 2);

    /* negative: variable argument should not specialize the same way */
    t = 3;
    neg = addk(7, t);

    /* safety: larger function call remains valid */
    safe = big_func(5);

    status = pos + neg + safe;
}
