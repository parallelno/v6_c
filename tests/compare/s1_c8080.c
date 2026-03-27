int main(int argc, char** argv) {
    char i;
    int z;

    argc = argc;
    argv = argv;

    i = 0;
    while (i < 16) {
        z = i * 3 + 1;
        i = i + 1;
    }

    return z;
}