; mul16.asm — 16-bit unsigned multiply for Intel 8080
;
; Entry:  DE = multiplicand, HL = multiplier  (same as codegen convention)
; Exit:   HL = DE * HL  (low 16 bits)
; Clobbers: A, B, C, D, E, flags

__mul16:
	MOV B,H		; BC = multiplier
	MOV C,L
	LXI H,0	; HL = result = 0
	MOV A,B		; check if multiplier is zero
	ORA C
	RZ		; if multiplier == 0, return 0
__mul16_loop:
	MOV A,C		; test low bit of BC
	RAR
	JNC __mul16_skip
	DAD D		; result += multiplicand
__mul16_skip:
	; shift multiplicand left (DE <<= 1)
	MOV A,E
	ADD A
	MOV E,A
	MOV A,D
	ADC A
	MOV D,A
	; shift multiplier right (BC >>= 1)
	MOV A,B
	ORA A
	RAR
	MOV B,A
	MOV A,C
	RAR
	MOV C,A
	; check if multiplier is zero
	ORA B
	JNZ __mul16_loop
	RET
