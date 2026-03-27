; div32.asm — 32-bit unsigned and signed divide/modulo for Intel 8080
;
; __div32u:  __op1 = __op1 / __op2  (unsigned)
; __mod32u:  __op1 = __op1 % __op2  (unsigned)
; __div32s:  __op1 = __op1 / __op2  (signed)
; __mod32s:  __op1 = __op1 % __op2  (signed)
;
; Entry:  __op1 (4 bytes) = dividend
;         __op2 (4 bytes) = divisor
; Exit:   __op1 (4 bytes) = result
;         HL = low 16 bits of result
; Clobbers: A, B, C, D, E, flags

; ---------------------------------------------------------------------------
; __div32u — unsigned 32-bit divide:  __op1 = __op1 / __op2
; ---------------------------------------------------------------------------
__div32u:
	CALL __divmod32u	; quotient in __dm32_q, remainder in __dm32_r
	; copy quotient to __op1
	LDA __dm32_q
	STA __op1
	LDA __dm32_q+1
	STA __op1+1
	LDA __dm32_q+2
	STA __op1+2
	LDA __dm32_q+3
	STA __op1+3
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __mod32u — unsigned 32-bit modulo:  __op1 = __op1 % __op2
; ---------------------------------------------------------------------------
__mod32u:
	CALL __divmod32u	; quotient in __dm32_q, remainder in __dm32_r
	; copy remainder to __op1
	LDA __dm32_r
	STA __op1
	LDA __dm32_r+1
	STA __op1+1
	LDA __dm32_r+2
	STA __op1+2
	LDA __dm32_r+3
	STA __op1+3
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __div32s — signed 32-bit divide
; ---------------------------------------------------------------------------
__div32s:
	; Determine result sign: XOR of sign bits
	LDA __op1+3
	MOV B,A
	LDA __op2+3
	XRA B
	PUSH PSW		; save result sign on stack
	; Make both operands positive
	CALL __abs32_op1
	CALL __abs32_op2
	CALL __divmod32u
	; copy quotient to __op1
	LDA __dm32_q
	STA __op1
	LDA __dm32_q+1
	STA __op1+1
	LDA __dm32_q+2
	STA __op1+2
	LDA __dm32_q+3
	STA __op1+3
	; negate if result should be negative
	POP PSW
	ORA A
	JP __div32s_done
	CALL __neg32_op1
__div32s_done:
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __mod32s — signed 32-bit modulo
; ---------------------------------------------------------------------------
__mod32s:
	; Result sign follows dividend sign
	LDA __op1+3
	PUSH PSW		; save dividend sign
	CALL __abs32_op1
	CALL __abs32_op2
	CALL __divmod32u
	; copy remainder to __op1
	LDA __dm32_r
	STA __op1
	LDA __dm32_r+1
	STA __op1+1
	LDA __dm32_r+2
	STA __op1+2
	LDA __dm32_r+3
	STA __op1+3
	POP PSW
	ORA A
	JP __mod32s_done
	CALL __neg32_op1
__mod32s_done:
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __abs32_op1 — make __op1 positive (negate if negative)
; ---------------------------------------------------------------------------
__abs32_op1:
	LDA __op1+3
	ORA A
	RP			; already positive
	; fall through to negate
__neg32_op1:
	LDA __op1
	CMA
	STA __op1
	LDA __op1+1
	CMA
	STA __op1+1
	LDA __op1+2
	CMA
	STA __op1+2
	LDA __op1+3
	CMA
	STA __op1+3
	; increment (add 1 for two's complement)
	LDA __op1
	ADI 1
	STA __op1
	LDA __op1+1
	ACI 0
	STA __op1+1
	LDA __op1+2
	ACI 0
	STA __op1+2
	LDA __op1+3
	ACI 0
	STA __op1+3
	RET

; ---------------------------------------------------------------------------
; __abs32_op2 — make __op2 positive (negate if negative)
; ---------------------------------------------------------------------------
__abs32_op2:
	LDA __op2+3
	ORA A
	RP
	LDA __op2
	CMA
	STA __op2
	LDA __op2+1
	CMA
	STA __op2+1
	LDA __op2+2
	CMA
	STA __op2+2
	LDA __op2+3
	CMA
	STA __op2+3
	LDA __op2
	ADI 1
	STA __op2
	LDA __op2+1
	ACI 0
	STA __op2+1
	LDA __op2+2
	ACI 0
	STA __op2+2
	LDA __op2+3
	ACI 0
	STA __op2+3
	RET

; ---------------------------------------------------------------------------
; __divmod32u — core unsigned 32-bit restoring division
;
;   Entry: __op1 = dividend, __op2 = divisor
;   Exit:  __dm32_q = quotient, __dm32_r = remainder
;   Clobbers: A, B, C, flags
; ---------------------------------------------------------------------------
__divmod32u:
	; Check for divide by zero
	LDA __op2
	MOV B,A
	LDA __op2+1
	ORA B
	MOV B,A
	LDA __op2+2
	ORA B
	MOV B,A
	LDA __op2+3
	ORA B
	JNZ __dm32_go
	; divide by zero: return 0
	XRA A
	STA __dm32_q
	STA __dm32_q+1
	STA __dm32_q+2
	STA __dm32_q+3
	STA __dm32_r
	STA __dm32_r+1
	STA __dm32_r+2
	STA __dm32_r+3
	RET
__dm32_go:
	; Clear remainder and quotient
	XRA A
	STA __dm32_r
	STA __dm32_r+1
	STA __dm32_r+2
	STA __dm32_r+3
	STA __dm32_q
	STA __dm32_q+1
	STA __dm32_q+2
	STA __dm32_q+3
	MVI A,32
	STA __dm32_cnt
__dm32_loop:
	; Shift quotient left by 1
	LDA __dm32_q
	ADD A
	STA __dm32_q
	LDA __dm32_q+1
	ADC A
	STA __dm32_q+1
	LDA __dm32_q+2
	ADC A
	STA __dm32_q+2
	LDA __dm32_q+3
	ADC A
	STA __dm32_q+3
	; Shift MSB of dividend into remainder
	LDA __op1
	ADD A
	STA __op1
	LDA __op1+1
	ADC A
	STA __op1+1
	LDA __op1+2
	ADC A
	STA __op1+2
	LDA __op1+3
	ADC A
	STA __op1+3	; carry = old MSB
	LDA __dm32_r
	RAL
	STA __dm32_r
	LDA __dm32_r+1
	RAL
	STA __dm32_r+1
	LDA __dm32_r+2
	RAL
	STA __dm32_r+2
	LDA __dm32_r+3
	RAL
	STA __dm32_r+3
	; Trial subtract: remainder - divisor
	; Save remainder first
	LDA __dm32_r
	STA __dm32_sv
	LDA __dm32_r+1
	STA __dm32_sv+1
	LDA __dm32_r+2
	STA __dm32_sv+2
	LDA __dm32_r+3
	STA __dm32_sv+3
	; remainder -= divisor
	LDA __dm32_r
	MOV B,A
	LDA __op2
	MOV C,A
	MOV A,B
	SUB C
	STA __dm32_r
	LDA __dm32_r+1
	MOV B,A
	LDA __op2+1
	MOV C,A
	MOV A,B
	SBB C
	STA __dm32_r+1
	LDA __dm32_r+2
	MOV B,A
	LDA __op2+2
	MOV C,A
	MOV A,B
	SBB C
	STA __dm32_r+2
	LDA __dm32_r+3
	MOV B,A
	LDA __op2+3
	MOV C,A
	MOV A,B
	SBB C
	STA __dm32_r+3
	JC __dm32_nfit
	; Fits: set quotient bit 0
	LDA __dm32_q
	ORI 1
	STA __dm32_q
	JMP __dm32_next
__dm32_nfit:
	; Doesn't fit: restore remainder
	LDA __dm32_sv
	STA __dm32_r
	LDA __dm32_sv+1
	STA __dm32_r+1
	LDA __dm32_sv+2
	STA __dm32_r+2
	LDA __dm32_sv+3
	STA __dm32_r+3
__dm32_next:
	LDA __dm32_cnt
	DCR A
	STA __dm32_cnt
	JNZ __dm32_loop
	RET

; Temporaries
__dm32_q:
	DS 4		; quotient
__dm32_r:
	DS 4		; remainder
__dm32_sv:
	DS 4		; saved remainder for restore
__dm32_cnt:
	DS 1		; loop counter

; Shared operand storage (used by mul32, div32, shift32)
__op1:
	DS 4
__op2:
	DS 4
