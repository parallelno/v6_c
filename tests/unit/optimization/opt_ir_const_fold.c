/* opt_ir_const_fold.c - IR constant folding/propagation coverage */

int status;
int pos;
int neg;
int safe;

void main(void)
{
    int a;
    int b;
    int z;

    /* positive: foldable constant expressions */
    pos = (8 + 4) * 2;
    pos = pos + (100 / 5);
    pos = pos + (33 % 8);
    pos = pos + ((7 & 3) | (8 ^ 1));

    /* negative: runtime values should not fully fold */
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

    status = pos + neg + safe;
}
