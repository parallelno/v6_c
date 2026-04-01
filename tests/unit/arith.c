/* arith.c — basic arithmetic test for v6c */

int add(int a, int b)
{
    return a + b;
}

int mul(int a, int b)
{
    return a * b;
}

int main(void)
{
    int x;
    int y;
    int result;

    x = 6;
    y = 7;

    if (mul(x, y) != 42) {
        return 1;
    }

    result = add(42, 8);
    if (result != 50) {
        return 2;
    }

    if (add(-1, 1) != 0) {
        return 3;
    }

    return 0;
}
