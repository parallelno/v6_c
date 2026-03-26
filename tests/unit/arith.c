/* arith.c — basic arithmetic test for v6c */

int result;

int add(int a, int b)
{
    return a + b;
}

int mul(int a, int b)
{
    return a * b;
}

void main(void)
{
    int x;
    int y;

    x = 6;
    y = 7;
    result = mul(x, y);       /* 42 */
    result = add(result, 8);  /* 50 */
}
