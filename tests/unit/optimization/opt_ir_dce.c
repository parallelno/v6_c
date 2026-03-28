/* opt_ir_dce.c - IR dead-code elimination coverage */

int status;
int side;

int dead_path(int x)
{
    if (x > 0) {
        return x + 1;
        x = x + 1000;
    }
    return x - 1;
}

void bump(void)
{
    side = side + 3;
}

void main(void)
{
    int pos;
    int neg;
    int safe;

    side = 0;

    /* positive: unreachable tail after return */
    pos = dead_path(5);

    /* negative: reachable branch should stay */
    if (pos > 0) {
        side = side + 1;
    }
    neg = side;

    /* safety: side-effecting call must remain even if return is unused */
    bump();
    safe = side;

    status = pos + neg + safe;
}
