/* opt_peephole_ctrl.c - peephole control-flow coverage */

int status;

int route(int x)
{
    if (x == 0) {
        goto L1;
    }
    goto L2;
L1:
    goto L3;
L2:
    return 9;
L3:
    return 7;
}

void main(void)
{
    int pos;
    int neg;
    int safe;

    /* positive: branch/jump patterns */
    pos = route(0);

    /* negative: alternate branch */
    neg = route(1);

    /* safety: direct compare path */
    if (neg > pos) {
        safe = neg - pos;
    } else {
        safe = pos - neg;
    }

    status = pos + neg + safe;
}
