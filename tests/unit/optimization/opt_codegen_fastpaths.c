/* opt_codegen_fastpaths.c - codegen helper fast-path coverage */

int status;

void main(void)
{
    int x;
    int pos;
    int neg;
    int safe;

    x = 9;

    /* positive: mul/div/mod/shift constants with known fast paths */
    pos = (x * 2) + (x * 3) + (x * 4) + (x * 8);
    pos = pos + (x / 1);
    pos = pos + (x % 1);
    pos = pos + (x % 2);
    pos = pos + (x << 1);
    pos = pos + (x >> 1);

    /* negative: generic cases */
    neg = (x * 7) + (x / 3) + (x % 3);

    /* safety: preserve signed behavior */
    safe = (-33) / 3;

    status = pos + neg + safe;
}
