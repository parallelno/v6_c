/* opt_peephole_ctrl.c - peephole control-flow coverage */

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

int main(void)
{
    int pos;
    int neg;
    int safe;

    /* pos = route(0): x==0 → L1→L3 → return 7 */
    pos = route(0);

    /* neg = route(1): x!=0 → L2 → return 9 */
    neg = route(1);

    /* neg>pos: 9>7 → safe = 9-7 = 2 */
    if (neg > pos) {
        safe = neg - pos;
    } else {
        safe = pos - neg;
    }

    if (pos != 7) return 1;
    if (neg != 9) return 2;
    if (safe != 2) return 3;
    return 0;
}
