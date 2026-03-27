    device zxspectrum48 ; There is no ZX Spectrum, it is needed for the sjasmplus assembler.
    org 100h
__begin:
__entry:
__init:
; 26 void __init() {
; 27     /* Zeroing uninitialized variables */
; 28     asm {

        ld   de, __bss
        xor  a
__init_loop:
        ld   (de), a
        inc  de
        ld   hl, 10000h - __end
        add  hl, de
        jp   nc, __init_loop

; 29         ld   de, __bss
; 30         xor  a
; 31 __init_loop:
; 32         ld   (de), a
; 33         inc  de
; 34         ld   hl, 10000h - __end
; 35         add  hl, de
; 36         jp   nc, __init_loop
; 37     }
; 38 
; 39     /* Init stack */
; 40 #if __has_include(<c8080/initstack.inc>) && !defined(ARCH_CPM_CCP) && !defined(ARCH_CPM_BDOS) && !defined(ARCH_CPM_BIOS)
; 41 #include <c8080/initstack.inc>
; 42 #endif
; 43 
; 44 #ifdef ARCH_CPM_CCP /* CCP remains in memory */
; 45     // clang-format off
; 46     asm {
; 47         pop  de
; 48         ld   a, (7)
; 49         sub  8
; 50         ld   h, a
; 51         ld   l, 0
; 52         ld   sp, hl
; 53         push de
; 54     }
; 55     // clang-format on
; 56 #endif
; 57 
; 58 #ifdef ARCH_CPM_BDOS /* BDOS remains in memory */
; 59     asm {
; 60         ld   a, (7)
; 61         ld   h, a
; 62         ld   l, 0
; 63         ld   sp, hl
; 64         ld   hl, 0
; 65         push hl
; 66     }
; 67 #endif
; 68 
; 69 #ifdef ARCH_CPM_BIOS /* BIOS remains in memory */
; 70 #error TODO
; 71 #endif
; 72 
; 73     main(0, NULL);
	ld hl, 0
	ld (__a_1_main), hl
main:
; 1 int main(int argc, char** argv) {
	ld (__a_2_main), hl
; 2     char i;
; 3     int z;
; 4 
; 5     argc = argc;
	ld hl, (__a_1_main)
	ld (__a_1_main), hl
; 6     argv = argv;
	ld hl, (__a_2_main)
	ld (__a_2_main), hl
; 7 
; 8     i = 0;
	xor a
	ld (main_i), a
; 9     while (i < 16) {
l_0:
	cp 16
	jp nc, l_1
; 10         z = i * 3 + 1;
	call __o_i8_to_i16
	ld de, 3
	call __o_mul_i16
	inc hl
	ld (main_z), hl
; 11         i = i + 1;
	ld a, (main_i)
	inc a
	ld (main_i), a
	jp l_0
l_1:
; 12     }
; 13 
; 14     return z;
	ld hl, (main_z)
	ret
__o_i8_to_i16:
; 222 void __o_i8_to_i16() {
; 223     asm {

        ld   l, a
        rla
        sbc  a
        ld   h, a

	ret
__o_mul_i16:
; 349 void __o_mul_i16() {
; 350     (void)__o_minus_16;
; 351     (void)__o_mul_u16;
; 352     asm {

        ld   a, h
        add  a
        jp   nc, __o_mul_i16_1  ; hl - positive

        call __o_minus_16

        ld   a, d
        add  a
        jp   nc, __o_mul_i16_2  ; hl - negative, de - positive

        ex   hl, de
        call __o_minus_16
        ex   hl, de

        jp   __o_mul_u16 ; hl & de - negative

__o_mul_i16_1:
        ld   a, d
        add  a
        jp   nc, __o_mul_u16  ; hl & de - positive

        ex   hl, de
        call __o_minus_16
        ex   hl, de

__o_mul_i16_2:
        call __o_mul_u16
        jp   __o_minus_16

	ret
__o_minus_16:
; 235 void __o_minus_16() {
; 236     asm {

        xor  a
        sub  l
        ld   l, a
        ld   a, 0
        sbc  h
        ld  h, a

	ret
__o_mul_u16:
; 326 void __o_mul_u16() {
; 327     asm {

        ld   b, h
        ld   c, l
        ld   hl, 0
        ld   a, 17
__o_mul_u16_l1:
        dec  a
        ret  z
        add  hl, hl
        ex   hl, de
        add  hl, hl
        ex   hl, de
        jp   nc, __o_mul_u16_l1
        add  hl, bc
        jp   __o_mul_u16_l1

	ret
__bss:
__static_stack:
	ds 7
__end:
__s___init equ __static_stack + 7
__s_main equ __static_stack + 0
__a_1_main equ __s_main + 3
__a_2_main equ __s_main + 5
main_i equ __s_main + 0
main_z equ __s_main + 1
__s___o_i8_to_i16 equ __static_stack + 0
__s___o_mul_i16 equ __static_stack + 0
__s___o_minus_16 equ __static_stack + 0
__s___o_mul_u16 equ __static_stack + 0
    savebin "tests/compare/s1_c8080.bin", __begin, __bss - __begin
