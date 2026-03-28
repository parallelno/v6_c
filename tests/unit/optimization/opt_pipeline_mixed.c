/* opt_pipeline_mixed.c - mixed pass interaction coverage */

int status;

int f(int x)
{
    return (x * 2) + 1;
}

void main(void)
{
    int i;
    int pos;
    int neg;
    int safe;

    /* positive: loop + small callee + const-like arithmetic */
    pos = 0;
    for (i = 0; i < 8; i = i + 1) {
        pos = pos + f(i);
    }

    /* negative: data-dependent branch inside loop */
    neg = 0;
    for (i = 0; i < 6; i = i + 1) {
        if ((i & 1) == 0) {
            neg = neg + i;
        } else {
            neg = neg - i;
        }
    }

    /* safety: call result used in compare */
    if (f(2) > 0) {
        safe = 5;
    } else {
        safe = 9;
    }

    status = pos + neg + safe;
}
