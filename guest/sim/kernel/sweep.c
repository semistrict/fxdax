// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The sweep (sweep.sh, TASK-149.15): every call the code under test makes that may fail on a real
 * kernel asks here first, a site each (sim_sweep_fails): allocations by fs/fxfs, fs/dax.c and the
 * XArrays and pool they use; the device's MAPs, UNMAPs and PRIVATEs; and Fxfs's room for dirty
 * pages, growths, writeback's commits, clones, seals and lookups. With SIM_SWEEP=N in the
 * environment, the Nth of them fails, once, as the run would have it fail at random; and the run
 * says as it ends (sim_end) how many it reached, so that a sweep runs it again for each N up to
 * that. Up to the Nth, the run is the run without it: a seed names the same run.
 *
 * Once the call has failed, what the test expects is void (sim_unexpected): the operation it was in
 * failed, and what came of that is the code's to say. What holds whatever fails stays checked:
 * ASan, UBSan, the code's WARNs, this kernel's checks, and the end of the run's (sim_end): the
 * filesystem synced, read as Fxfs holds it, what tasks were told durable, unmounted, nothing leaked.
 */

#include <sim/sim.h>
#include <sim/host.h>

/*
 * The call to fail, from 1, or 0; how many calls have asked; whether the one to fail has; and
 * whether the end of a run's checks are reading what they check, which no call of fails.
 */
static u64 sweep_at, sweep_calls, failed_ns;
static bool sweep_failed, paused;
static unsigned int voided;

/*
 * How long a run may go on once the sweep's call has failed, in virtual time: past the longest
 * dax.c waits for room before it gives up (FXFS_DAX_NO_ROOM_WAITS of 10 ms), many times over.
 */
#define SWEEP_AFTER_NS (300ULL * NSEC_PER_SEC)

static int __init init_sweep(void)
{
	const char *at = host_getenv("SIM_SWEEP");

	if (at && sscanf(at, "%llu", &sweep_at) != 1)
		sim_panic("SIM_SWEEP=%s isn't a number", at);
	return 0;
}
fs_initcall(init_sweep);

bool sim_sweep_fails(const char *what, const char *file, int line)
{
	if (paused || ++sweep_calls != sweep_at)
		return false;
	sweep_failed = true;
	failed_ns = ktime_get_ns();
	sim_log("sim: sweep: call %llu fails: %s at %s:%d, by %s, at %llu points\n", sweep_calls,
		what, file, line, current ? current->comm : "?", sim_points());
	host_stack_trace();
	return true;
}

void sim_sweep_pause(bool pause)
{
	paused = pause;
}

/*
 * A run still going SWEEP_AFTER_NS after the sweep's call failed, every task asleep, one of them
 * at a pause point (fxfs.dax_pause), or a hold (a commit's, a seal's, a MAP's), its test can't let
 * go of, kept from it by what failed, as a write's room made by syncing waits for the writeback
 * held: it ends there, void. One with none goes on, a deadlock's or a livelock's to stop, as
 * dax.c's waits for room give up after 30 s each, a write at a time.
 */
void sim_sweep_watch(u64 now)
{
	if (!sweep_failed || now - failed_ns < SWEEP_AFTER_NS)
		return;
	if (!sim_param_read("dax_paused") && !fxfs_sim_commit_held() && !fxfs_sim_seal_held() &&
	    !sim_device_held())
		return;
	sim_log("sim: sweep: a task waits at a pause point, or a hold, its test can't let go of, %llu s after call %llu failed: void, the run ends\n",
		SWEEP_AFTER_NS / NSEC_PER_SEC, sweep_at);
	sim_end_void();
	host_end(0);
}

bool sim_swept(void)
{
	return sweep_failed;
}

u64 sim_sweep_calls(void)
{
	return sweep_calls;
}

void sim_unexpected(const char *format, ...)
{
	char line[1024];
	va_list args;

	va_start(args, format);
	vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	if (!sweep_failed)
		sim_panic("%s", line);
	if (voided++ < 8)
		sim_log("sim: sweep: void, as call %llu failed before it: %s\n", sweep_at, line);
}
