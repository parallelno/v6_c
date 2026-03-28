/* opt_ir_strength.c - IR strength reduction coverage */

int status;
int x;
int pos;
int neg;
unsigned int ux;
int safe;

void main(void)
{
    x = 9;

    /* positive: power-of-two multipliers */
    pos = (x * 8) + (x * 4) + (x * 2);

    /* negative: non-power-of-two multiply should stay generic */
    neg = x * 7;

    /* safety: unsigned div/mod by power of two */
    ux = 123;
    safe = (int)(ux / 8);
    safe = safe + (int)(ux % 8);

    status = pos + neg + safe;
}
