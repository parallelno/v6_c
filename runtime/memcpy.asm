; memcpy.asm — memory operations for Intel 8080
;
; Minimal implementations for the C standard library.
; The v6c codegen does not call these directly in Phase 1
; but they are part of the runtime library for user programs.
;
; Clobbers: A, B, C, D, E, H, L, flags

; ---------------------------------------------------------------------------
; memcpy — copy BC bytes from (DE) to (HL)
;   Entry: HL = dest, DE = src, BC = count
;   Exit:  HL = original dest
; ---------------------------------------------------------------------------
memcpy:
	MOV A,B
	ORA C
	RZ			; count == 0 → done
	PUSH H			; save dest for return
__memcpy_loop:
	LDAX D			; A = *(src)
	MOV M,A			; *(dest) = A
	INX H
	INX D
	DCX B
	MOV A,B
	ORA C
	JNZ __memcpy_loop
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; memset — fill BC bytes at (HL) with value A
;   Entry: HL = dest, A = value, BC = count
;   Exit:  HL = original dest
; ---------------------------------------------------------------------------
memset:
	PUSH H			; save dest for return
	PUSH PSW		; save fill value
	MOV A,B
	ORA C
	JZ __memset_done
	POP PSW
	PUSH PSW
__memset_loop:
	MOV M,A
	INX H
	DCX B
	MOV D,A			; preserve fill value (B may change)
	MOV A,B
	ORA C
	MOV A,D
	JNZ __memset_loop
__memset_done:
	POP PSW			; discard saved value
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; strlen — return length of null-terminated string at (HL)
;   Entry: HL = string pointer
;   Exit:  HL = length (16-bit)
; ---------------------------------------------------------------------------
strlen:
	LXI B,0		; BC = count = 0
__strlen_loop:
	MOV A,M
	ORA A
	JZ __strlen_done
	INX H
	INX B
	JMP __strlen_loop
__strlen_done:
	MOV H,B
	MOV L,C
	RET

; ---------------------------------------------------------------------------
; strcmp — compare two null-terminated strings
;   Entry: HL = s1, DE = s2
;   Exit:  HL = <0 if s1<s2, 0 if equal, >0 if s1>s2
; ---------------------------------------------------------------------------
strcmp:
__strcmp_loop:
	LDAX D			; A = *s2
	MOV B,A			; B = *s2
	MOV A,M			; A = *s1
	CMP B			; compare *s1 with *s2
	JNZ __strcmp_diff
	ORA A			; check for null terminator
	JZ __strcmp_equal
	INX H
	INX D
	JMP __strcmp_loop
__strcmp_diff:
	; A = *s1, B = *s2. Return *s1 - *s2 sign-extended to 16 bits.
	SUB B			; A = *s1 - *s2
	MOV L,A
	MVI H,0
	ORA A
	RP			; positive → done
	MVI H,0xFF		; negative → sign-extend
	RET
__strcmp_equal:
	LXI H,0
	RET
