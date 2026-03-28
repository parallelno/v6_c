/* opt_ir_lsf.c - IR load/store forwarding coverage */

int status;
int g0;
int g1;
int g2;
int pos;
int neg;
int safe;

int touch(int v)
{
    g2 = v + 1;
    return g2;
}

void main(void)
{
    int *p;

    /* positive: store then load from same global slot */
    g0 = 10;
    g1 = g0;
    pos = g1;

    /* negative: different slot should not forward */
    g0 = 3;
    g1 = 4;
    neg = g0 + g1;

    /* safety: pointer store/call should invalidate forwarding facts */
    p = &g0;
    *p = 20;
    safe = g0;
    safe = safe + touch(5);
    g1 = g0;
    safe = safe + g1;

    status = pos + neg + safe;
}
