/* opt_peephole_data.c - peephole data-movement coverage */

int status;
int g;

void main(void)
{
    int x;
    int y;
    int pos;
    int neg;
    int safe;

    /* positive: redundant copy-style updates */
    x = 3;
    g = x;
    g = g;
    pos = g;

    /* negative: distinct values */
    y = 5;
    neg = x + y;

    /* safety: preserve observable store */
    g = neg;
    safe = g;

    status = pos + neg + safe;
}
