@memusage_v6_macros:
.macro HLT_(i)
		.loop i
			hlt
		.endloop
.endmacro

.macro RRC_(i)
		.loop i
			rrc
		.endloop
.endmacro

.macro RAL_(i)
		.loop i
			ral
		.endloop
.endmacro

.macro RLC_(i)
		.loop i
			rlc
		.endloop
.endmacro

.macro PUSH_B(i)
		.loop i
			push b
		.endloop
.endmacro

.macro PUSH_H(i)
		.loop i
			push h
		.endloop
.endmacro

.macro POP_H(i)
		.loop i
			pop h
		.endloop
.endmacro

.macro INR_D(i)
		.loop i
			inr d
		.endloop
.endmacro

.macro INX_H(i)
		.loop i
			inx h
		.endloop
.endmacro

.macro INX_D(i)
		.loop i
			inx d
		.endloop
.endmacro

.macro DCX_H(i)
		.loop i
			dcx h
		.endloop
.endmacro

.macro DCR_M(i)
		.loop i
			dcr m
		.endloop
.endmacro

.macro INR_L(i)
		.loop i
			inr l
		.endloop
.endmacro

.macro INR_H(i)
		.loop i
			inr h
		.endloop
.endmacro

.macro INR_M(i)
		.loop i
			inr m
		.endloop
.endmacro

.macro INR_A(i)
		.loop i
			inr a
		.endloop
.endmacro

.macro NOP_(i)
		.loop i
			nop
		.endloop
.endmacro

.macro ADD_A(i)
		.loop i
			add a
		.endloop
.endmacro

.macro DAD_H(i)
		.loop i
			dad h
		.endloop
.endmacro

.macro LXI_B(val)
	.if val < 0
		lxi b, $ffff + val + 1
	.endif
	.if val >= 0
		lxi b, val
	.endif
.endmacro

.macro LXI_D(val)
	.if val < 0
		lxi d, $ffff + val + 1
	.endif
	.if val >= 0
		lxi d, val
	.endif
.endmacro

.macro LXI_H(val)
	.if val < 0
		lxi h, $ffff + val + 1
	.endif
	.if val >= 0
		lxi h, val
	.endif
.endmacro

.macro MVI_A_TO_DIFF(offset_from, offset_to)
		offset_addr = offset_to - offset_from
		.if offset_addr > 0
			mvi a, <offset_addr
		.endif
		.if offset_addr < 0
			mvi a, <($ffff + offset_addr + 1)
		.endif
.endmacro

.macro LXI_B_TO_DIFF(offset_from, offset_to)
		offset_addr = offset_to - offset_from
		.if offset_addr > 0
			lxi b, offset_addr
		.endif
		.if offset_addr < 0
			lxi b, $ffff + offset_addr + 1
		.endif
.endmacro

.macro LXI_D_TO_DIFF(offset_from, offset_to)
		offset_addr = offset_to - offset_from
		.if offset_addr > 0
			lxi d, offset_addr
		.endif
		.if offset_addr < 0
			lxi d, $ffff + offset_addr + 1
		.endif
.endmacro

.macro LXI_H_TO_DIFF(offset_from, offset_to)
		offset_addr = offset_to - offset_from
		.if offset_addr > 0
			lxi h, offset_addr
		.endif
		.if offset_addr < 0
			lxi h, $ffff + offset_addr + 1
		.endif
.endmacro

; it advances HL by the diff equals to (addr_to - addr_from)
; 8-24 cc if reg_pair is not provided. It uses:
; 		INX_H(N)/DCX_H(N).
; 24 cc if reg_pair = BY_BC/BY_DE/BY_HL_FROM_BC/BY_HL_FROM_DE. It uses:
; 		lxi reg_pair; dad reg_pair.
; 40 cc if reg_pair = BY_A. It uses:
;		mvi a, <diff_addr
;		add l
;		mov l, a
;		mvi a, >diff_addr
;		adc h
;		mov h, a

; it validates the diff suggesting improvements
; use:
; hl, reg_pair
; cc:
; reg_pair:
BY_BC			= 1
BY_DE			= 2
BY_HL_FROM_BC	= 3
BY_HL_FROM_DE	= 4
BY_A			= 5
.macro HL_ADVANCE(addr_from, addr_to, reg_pair = NULL)
		diff_addr = addr_to - addr_from

		.if reg_pair == NULL
			.if diff_addr > 0
				INX_H(diff_addr)
			.endif
			.if diff_addr < 0
				DCX_H(-diff_addr)
			.endif
			; validation
			.if diff_addr < -3 || diff_addr > 3
				.error "HL_ADVANCE(", addr_from, ", ", addr_to, ") with diff (", diff_addr, ") is outside of the required range of [-3, 3]. Use BY_BC, BY_DE, BY_HL_FROM_BC or BY_HL_FROM_DE as the third argument."
			.endif
		.endif

		.if reg_pair == BY_BC
				LXI_B_TO_DIFF(addr_from, addr_to)
				dad b
		.endif
		.if reg_pair == BY_DE
				LXI_D_TO_DIFF(addr_from, addr_to)
				dad d
		.endif
		.if reg_pair == BY_HL_FROM_BC
				LXI_H_TO_DIFF(addr_from, addr_to)
				dad b
		.endif
		.if reg_pair == BY_HL_FROM_DE
				LXI_H_TO_DIFF(addr_from, addr_to)
				dad d
		.endif
		.if reg_pair == BY_A
			mvi a, <diff_addr
			add l
			mov l, a
			mvi a, >diff_addr
			adc h
			mov h, a
		.endif
		.if (reg_pair == BY_BC || reg_pair == BY_DE )  && diff_addr >= -3 && diff_addr <= 3
			.error "HL_ADVANCE(" addr_from ", " addr_to", BY_BC/BY_DE) with diff (" diff_addr ") is in too short range [-3, 3]. Keep the third argument undefined."
		.endif
.endmacro

.macro DE_ADVANCE(addr_from, addr_to)
		diff_addr = addr_to - addr_from

		.if diff_addr > 0
			INX_D(diff_addr)
		.endif
		.if diff_addr < 0
			DCX_D(-diff_addr)
		.endif
		; validation
		.if diff_addr < -3 || diff_addr > 3
			.error "DE_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") is outside of the required range of [-3, 3]."
		.endif
.endmacro

; bc += hl
; uses: a
; 40cc
.macro BC_TO_BC_PLUS_HL()
			mov a, c
			add l
			mov c, a
			mov a, b
			adc h
			mov b, a
.endmacro

; bc += de
; uses: a
; 40cc
.macro BC_TO_BC_PLUS_DE()
			mov a, c
			add e
			mov c, a
			mov a, b
			adc d
			mov b, a
.endmacro


.macro LXI_H_NEG(val)
		lxi h, $ffff - val + 1
.endmacro

.macro LXI_D_NEG(val)
		lxi d, $ffff - val + 1
.endmacro

; to replace xra a with a meaningful macro + constant
.macro A_TO_ZERO(int8_const, useXRA = true)
		.if int8_const != 0
			.error "A_TO_ZERO macros was used with a non-zero constant = ", int8_const
		.endif
		.if useXRA
			xra a
		.endif
		.if useXRA == false
			mvi a, 0
		.endif
.endmacro

; hl = a + int16_const
; 36 cc
.macro HL_TO_A_PLUS_INT16(int16_const)
			adi <int16_const
			mov l, a
			aci >int16_const
			sub l
			mov h, a
.endmacro

; bc = a + int16_const
; 36 cc
.macro BC_TO_A_PLUS_INT16(int16_const)
			adi <int16_const
			mov c, a
			aci >int16_const
			sub c
			mov b, a
.endmacro

; bc = a * 2 + int16_const
; cc = 40
.macro BC_TO_AX2_PLUS_INT16(int16_const)
			add a
			adi <int16_const
			mov c, a
			aci >int16_const
			sub c
			mov b, a
.endmacro
; de = a * 2 + int16_const
; cc = 40
.macro DE_TO_AX2_PLUS_INT16(int16_const)
			add a
			adi <int16_const
			mov e, a
			aci >int16_const
			sub e
			mov d, a
.endmacro
; hl = a * 2 + int16_const
; cc = 40
.macro HL_TO_AX2_PLUS_INT16(int16_const)
			add a
			adi <int16_const
			mov l, a
			aci >int16_const
			sub l
			mov h, a
.endmacro

; cc = 44
.macro HL_TO_AX4_PLUS_INT16(int16_const)
			ADD_A(2)
			adi <int16_const
			mov l, a
			aci >int16_const
			sub l
			mov h, a
.endmacro

.macro CPI_ZERO(int8_const = 0)
		.if int8_const != 0
			.error "CPI_ZERO macros was used with a non-zero constant = ", int8_const
		.endif
		ora a
.endmacro

;===============================================================================
; Ram Mapping
; ALL RAM_DISK_* macros has to be placed BEFORE lxi sp, *, and sphl!

; restore the RAM Disk mode
; usually used in the interruption routine
.macro RAM_DISK_RESTORE(ram_disk_port = RAM_DISK_PORT)
			lda ram_disk_mode
			out ram_disk_port
.endmacro

; mount the RAM Disk w/o storing mode
.macro RAM_DISK_ON_NO_RESTORE(command, ram_disk_port = RAM_DISK_PORT)
			mvi a, <command
			out ram_disk_port
.endmacro

; mount the RAM Disk
; command is a RAM Disk activation command
; call, push, pop operations are prohibited after this macro until RAM_DISK_OFF
.macro RAM_DISK_ON(command, ram_disk_port = RAM_DISK_PORT)
			mvi a, <command
			sta ram_disk_mode
			out ram_disk_port
.endmacro

; mount the RAM Disk
; in:
; a - RAM Disk activation command
.macro RAM_DISK_ON_BANK(ram_disk_port = RAM_DISK_PORT)
			sta ram_disk_mode
			out ram_disk_port
.endmacro

; mount the RAM Disk w/o storing a mode
.macro RAM_DISK_ON_BANK_NO_RESTORE(ram_disk_port = RAM_DISK_PORT)
			out ram_disk_port
.endmacro

; dismount the RAM Disk
; lxi sp, RESTORE_SP is required after this macro
; 8*4=32 cc
.macro RAM_DISK_OFF(useXRA = true, ram_disk_port = RAM_DISK_PORT)
			A_TO_ZERO(RAM_DISK_OFF_CMD, useXRA)
			sta ram_disk_mode
			out ram_disk_port
.endmacro

; dismount the RAM Disk w/o storing mode
; should be used inside the interruption call or with disabled interruptions
.macro RAM_DISK_OFF_NO_RESTORE(useXRA = true, ram_disk_port = RAM_DISK_PORT)
			A_TO_ZERO(RAM_DISK_OFF_CMD, useXRA)
			out ram_disk_port
.endmacro

;===============================================================================
; Functions that access the RAM Disk data
.macro CALL_RAM_DISK_FUNC(func_addr, _command, disable_int = false, useXRA = true)
		.if disable_int
			di
		.endif
			RAM_DISK_ON(_command)
			call func_addr
			RAM_DISK_OFF(useXRA)
		.if disable_int
			ei
		.endif
.endmacro

; a - RAM Disk activation command
.macro CALL_RAM_DISK_FUNC_BANK(func_addr, disable_int = false, useXRA = true)
		.if disable_int
			di
		.endif
			RAM_DISK_ON_BANK()
			call func_addr
			RAM_DISK_OFF(useXRA)
		.if disable_int
			ei
		.endif
.endmacro

.macro CALL_RAM_DISK_FUNC_NO_RESTORE(func_addr, _command, disable_int = false, useXRA = true)
		.if disable_int
			di
		.endif
			RAM_DISK_ON_NO_RESTORE(_command)
			call func_addr
			RAM_DISK_OFF_NO_RESTORE(useXRA)
		.if disable_int
			ei
		.endif
.endmacro

;===============================================================================

.macro DEBUG_BORDER_LINE(_borderColorIdx = 1)
		.if SHOW_CPU_HIGHLOAD_ON_BORDER
			mvi a, PORT0_OUT_OUT
			out 0
			mvi a, _borderColorIdx
			out 2
			lda scr_offset_y
			out 3
		.endif
.endmacro

.macro DEBUG_HLT()
		.if SHOW_CPU_HIGHLOAD_ON_BORDER
			hlt
		.endif
.endmacro

; for a jmp table with 4 byte allignment
.macro JMP_4(DST_ADDR)
			jmp DST_ADDR
			nop
.endmacro
; for a jmp table with 4 byte allignment
.macro RET_4() ;
			ret
			NOP_(3)
.endmacro

.macro CLAMP_A(val_max = $ff)
			cpi val_max + 1
			jc @no_clamp
			mvi a, val_max
@no_clamp:
.endmacro

.macro CLAMP_M(val_max = $ff)
			mov a, m
			cpi val_max + 1
			jc @no_clamp
			mvi m, val_max
@no_clamp:
.endmacro

; cc often 40
; cc rare 28
.macro INR_CLAMP_M(val_max = $ff)
			mov a, m
			cpi val_max
			jz @clamp
			inr m
@clamp:
.endmacro

; cc often 48
; cc rare 40
.macro INR_WRAP_M(val_max = $ff, no_wrap)
			inr m
			mvi a, val_max
			sub m
			jnz no_wrap
			mov m, a
.endmacro

; rev1: 4 cc
.macro SET_CY(val)
		.if val == 0
			ora a ; set CY = 0
		.endif
		.if val != 1
			stc ; set CY = 1
		.endif
.endmacro

 ; ints_per_update = 2 means the update happens every second interruption (25 updates per second)

;===============================================================================
; Game Update Counter Check
;===============================================================================
; Waits for the required number of interrupts before allowing a game update.
; Note:
; - Throttls the update loop (e.g. from 50Hz down to 25Hz).
; - Should be called at the start of the game_update() function.
; - Uses the game_updates_required counter incremented in the interruption routine.
; - The default number of interrupts per game update is 2.
;
; Parameters:
; game_updates_required - pts to pending game updates counter
; ints_per_update        - Number of interrupts per game update
;
.macro CHECK_GAME_UPDATE_COUNTER(game_updates_required, ints_per_update = 2)
			; check if an interruption happened
			lxi h, game_updates_required
			mov a, m
			ora a
			rm
			DCR_M(ints_per_update)
.endmacro

.macro TEXT(string, end_code = _EOD_)
.encoding "screencodecommodore", "mixed"
			.text string
			.byte end_code
.endmacro
