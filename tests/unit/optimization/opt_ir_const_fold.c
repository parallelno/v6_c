/* opt_ir_const_fold.c - IR constant folding/propagation coverage */

int main(void)
{
    int pos;
    int neg;
    int safe;
    int a;
    int b;
    int z;

    /* positive: foldable constant expressions */
    /* (8+4)*2=24, +20=44, +1=45, +(3|9)=56 */
    pos = (8 + 4) * 2;
    pos = pos + (100 / 5);
    pos = pos + (33 % 8);
    pos = pos + ((7 & 3) | (8 ^ 1));

    /* negative: runtime values: 7*3+7/3 = 21+2 = 23 */
    a = 7;
    b = 3;
    neg = (a * b) + (a / b);

    /* safety: guard divide-by-zero path */
    z = 0;
    if (z != 0) {
        safe = 10 / z;
    } else {
        safe = 123;
    }

    if (pos != 56) return 1;
    if (neg != 23) return 2;
    if (safe != 123) return 3;
    return 0;
}
