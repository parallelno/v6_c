/* opt_ir_dce.c - IR dead-code elimination coverage */

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

int main(void)
{
    int pos;
    int neg;
    int safe;

    side = 0;

    /* positive: dead_path(5): 5>0 → return 6 */
    pos = dead_path(5);

    /* negative: pos>0 → side=1; neg=side=1 */
    if (pos > 0) {
        side = side + 1;
    }
    neg = side;

    /* safety: bump() → side=4; safe=4 */
    bump();
    safe = side;

    if (pos != 6) return 1;
    if (neg != 1) return 2;
    if (safe != 4) return 3;
    return 0;
}
