#!/usr/bin/env python3
"""Two-arm equality experiment; Linux, Python stdlib, pinned Rust + native linker."""
import argparse, hashlib, json, math, os, pathlib, platform, random, resource
import statistics as st
import subprocess as sp
import time

# Consecutive commits on main that differ by exactly the depth bound (#29):
# de3c792 is its squash and f2f2b40 its parent. The branch commits this was
# first written against would not have survived the branch being deleted.
ARMS = {'baseline': 'f2f2b40cb501dcefbdb1f69ec5735fba92513a5e',
        'bounded': 'de3c792ad74827c79c76f6ebde32add65be9ea19'}
ROOT = pathlib.Path(__file__).resolve().parents[2]

def command(args, cwd=None):
    return sp.check_output(args, cwd=cwd, stderr=sp.STDOUT, text=True).strip()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def summary(xs):
    xs = sorted(xs)
    return {'n': len(xs), 'median': st.median(xs),
            'p99': xs[math.ceil(.99 * len(xs))-1],
            'sample_variance': st.variance(xs) if len(xs)>1 else None,
            'min': xs[0], 'max': xs[-1]}

def ticks():
    return list(map(int, pathlib.Path('/proc/stat').read_text().splitlines()[0].split()[1:9]))

def activity(a, b):
    d = [y-x for x,y in zip(a,b)]
    total = sum(d)
    return {'ticks': d, 'busy_pct': 100*(total-d[3]-d[4]-d[7])/total,
            'iowait_pct':100*d[4]/total, 'steal_pct':100*d[7]/total} if total else None

def sources(out):
    # Independent objects: equality cannot return early on unequal pointers.
    lines = ['const N0 = struct { value: i64 };']
    lines += [f'const N{i} = struct {{ child: N{i-1} }};' for i in range(1,16)]
    lines += ['fn main() i64 {', 'var a0 = N0{ .value = 7 };', 'var b0 = N0{ .value = 7 };']
    for i in range(1,16):
        lines += [f'const a{i} = N{i}{{ .child = a{i-1} }};', f'const b{i} = N{i}{{ .child = b{i-1} }};']
    lines += ['var hits: i64 = 0;', 'var i: i64 = 0;',
              'while (i < 2000000) : (i += 1) {', 'b0.value = 7 + i % 2;',
              'if (a15 == b15) { hits += 1; }', '}', 'print(hits);', 'return 0;', '}']
    micro = '\n'.join(lines)+'\n'
    whole = '''// Synthetic record reconciliation: allocations, arithmetic and depth-3 equality.
const Key = struct { id: i64, region: i64 };
const Payload = struct { key: Key, value: i64 };
const Record = struct { payload: Payload, version: i64 };
fn main() i64 {
    var i: i64 = 0;
    var hits: i64 = 0;
    var checksum: i64 = 0;
    while (i < 20000) : (i += 1) {
        var value: i64 = i;
        var j: i64 = 0;
        while (j < 100) : (j += 1) { value = (value * 17 + j) % 1000003; }
        const a = Record{ .payload = Payload{ .key = Key{ .id = i, .region = i % 8 }, .value = value }, .version = 1 };
        const b = Record{ .payload = Payload{ .key = Key{ .id = i, .region = i % 8 }, .value = value + i % 2 }, .version = 1 };
        if (a == b) { hits += 1; }
        checksum += value;
    }
    print(hits);
    print(checksum);
    return 0;
}
'''
    value_sum = 0
    for i in range(20000):
        v = i
        for j in range(100): v = (v*17+j)%1000003
        value_sum += v
    expected = {'micro': '1000000\n', 'whole': f'10000\n{value_sum}\n'}
    for name, src in [('micro', micro), ('whole', whole)]:
        (out/f'{name}.ws').write_text(src)
    return expected

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', required=True, type=pathlib.Path)
    p.add_argument('--arms-root', type=pathlib.Path, help='reuse clean exact-SHA checkouts; still verifies locked release build')
    p.add_argument('--blocks', type=int, default=10)
    p.add_argument('--per-block', type=int, default=10)
    p.add_argument('--noise-blocks', type=int, default=4)
    p.add_argument('--quiet-seconds', type=int, default=60)
    p.add_argument('--smoke', action='store_true', help='validation only: no statistical conclusion')
    args=p.parse_args()
    if args.smoke: args.blocks,args.per_block,args.noise_blocks,args.quiet_seconds=1,1,2,2
    if min(args.blocks,args.per_block,args.noise_blocks,args.quiet_seconds)<1: p.error('counts must be positive')
    out=args.output.resolve(); out.mkdir(parents=True, exist_ok=False)
    expected=sources(out)
    env=os.environ.copy()
    for k in list(env):
        if k.startswith('WSHARP_') or k in ('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS'): del env[k]
    meta={'schema':1,'utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),
          'harness_sha':command(['git','rev-parse','HEAD'],ROOT),
          'harness_dirty':command(['git','status','--porcelain'],ROOT),
          'harness_file_sha256':digest(pathlib.Path(__file__)),
          'command': __import__('sys').argv, 'config':vars(args).copy(),
          'uname':list(platform.uname()),'os_release':pathlib.Path('/etc/os-release').read_text(),
          'lscpu':command(['lscpu','-J']), 'meminfo':pathlib.Path('/proc/meminfo').read_text(),
          'python':platform.python_version(),'rustc':command(['rustc','+1.95.0','-Vv']),
          'cargo':command(['cargo','+1.95.0','-V']), 'cc':command(['cc','--version']),
          'ld':command(['ld','--version']), 'arms':{},
          'workload_sha256':{n:digest(out/f'{n}.ws') for n in expected},
          'allocation_bytes':'not instrumented in these historical arms',
          'regimes':{'aot':'fresh AOT process; compile excluded; 3 untimed process runs per arm/workload',
                     'jit':'fresh compiler process each sample; includes JIT compilation and execution; no in-process JIT warmup'}}
    def save(): (out/'metadata.json').write_text(json.dumps(meta,indent=2,default=str)+'\n')
    save()
    commands={}
    for arm,sha in ARMS.items():
        tree=(args.arms_root/sha).resolve() if args.arms_root else out/'arms'/sha
        if not args.arms_root:
            tree.parent.mkdir(exist_ok=True)
            command(['git','clone','--no-hardlinks',str(ROOT),str(tree)])
            command(['git','checkout','--detach',sha],tree)
        assert command(['git','rev-parse','HEAD'],tree)==sha
        assert not command(['git','status','--porcelain','--untracked-files=no'],tree), 'dirty arm'
        with (out/f'{arm}-build.log').open('w') as log:
            sp.run(['cargo','+1.95.0','build','--release','--locked'],cwd=tree,env=env,stdout=log,stderr=sp.STDOUT,check=True)
        binary=tree/'target/release/wsharp'
        meta['arms'][arm]={'sha':sha,'tree':str(tree),'binary_sha256':digest(binary),
                           'lock_sha256':digest(tree/'Cargo.lock'),'profile':'release default; no RUSTFLAGS'}
        for name in expected:
            exe=out/f'{arm}-{name}'
            build=[str(binary),'build',str(out/f'{name}.ws'),'-o',str(exe)]
            with (out/f'{arm}-{name}-aot.log').open('w') as log:
                sp.run(build,cwd=tree,env=env,stdout=log,stderr=sp.STDOUT,check=True)
            meta['arms'][arm][name+'_aot_sha256']=digest(exe)
            commands[arm,name,'aot']=([str(exe)],tree)
            commands[arm,name,'jit']=([str(binary),'run',str(out/f'{name}.ws')],tree)
        save()
    raw=(out/'samples.jsonl').open('w',buffering=1)
    def sample(arm,name,mode,phase,block,index,label):
        cmd,cwd=commands[arm,name,mode]
        a=ticks(); start=time.perf_counter_ns()
        # wait4 returns per-child RSS, unlike cumulative RUSAGE_CHILDREN.ru_maxrss.
        with (out/'child.stdout').open('wb') as stdout, (out/'child.stderr').open('wb') as stderr:
            child=sp.Popen(cmd,cwd=cwd,env=env,stdout=stdout,stderr=stderr)
            _,status,usage=os.wait4(child.pid,0)
            child.returncode=os.waitstatus_to_exitcode(status)
        elapsed=time.perf_counter_ns()-start; b=ticks()
        stdout=(out/'child.stdout').read_text(); stderr=(out/'child.stderr').read_text()
        row={'arm':arm,'workload':name,'mode':mode,'phase':phase,'block':block,'index':index,'label':label,
             'elapsed_ns':elapsed,'peak_rss_kib':usage.ru_maxrss,'user_s':usage.ru_utime,'system_s':usage.ru_stime,
             'exit_code':child.returncode,'stdout':stdout,'stderr':stderr,'activity':activity(a,b)}
        raw.write(json.dumps(row)+'\n')
        if child.returncode or stdout!=expected[name]: raise RuntimeError(f'workload failed: {row}')
        return row
    # Warm pages/allocator process startup, but never claim in-process JIT steady state.
    for name in expected:
        for mode in ['aot','jit']:
            for arm in ARMS:
                for i in range(3): sample(arm,name,mode,'warmup',0,i,arm)
    quiet=[]
    for i in range(args.quiet_seconds):
        a=ticks(); time.sleep(1); quiet.append(activity(a,ticks()))
    # A second in which no tick moved says nothing about load, so it fails the gate.
    gate=all(x is not None and x['busy_pct']<5 and x['iowait_pct']<1 and x['steal_pct']<1 for x in quiet)
    (out/'quiet.json').write_text(json.dumps({'passed':gate,'samples':quiet},indent=2)+'\n')
    if not gate and not args.smoke: raise RuntimeError('quiet gate failed; retain this attempt and rerun in a new output directory')
    rows=[]
    rng=random.Random(19)
    for phase,blocks in [('noise',args.noise_blocks),('comparison',args.blocks)]:
        for block in range(blocks):
            for name in expected:
                for mode in ['aot','jit']:
                    for i in range(args.per_block):
                        labels=['baseline','bounded']; rng.shuffle(labels)
                        for label in labels:
                            arm='baseline' if phase=='noise' else label
                            rows.append(sample(arm,name,mode,phase,block,i,label))
            print(json.dumps({'phase':phase,'block_complete':block+1}),flush=True)
    report={'units':'ns (variance ns^2); RSS KiB','percentile':'nearest-rank; median midpoint',
            'smoke':args.smoke,'results':[]}
    for name in expected:
        for mode in ['aot','jit']:
            result={'workload':name,'mode':mode}
            for phase in ['noise','comparison']:
                subset=[r for r in rows if r['workload']==name and r['mode']==mode and r['phase']==phase]
                result[phase]={}
                medians={}
                for label in ARMS:
                    group=[r for r in subset if r['label']==label]
                    medians[label]=[st.median([r['elapsed_ns'] for r in group if r['block']==b]) for b in sorted({r['block'] for r in group})]
                    result[phase][label]={'elapsed':summary([r['elapsed_ns'] for r in group]),
                        'peak_rss_kib':summary([r['peak_rss_kib'] for r in group]),
                        'block_medians_ns':medians[label], 'across_block_medians':summary(medians[label])}
                deltas=[100*(b/a-1) for a,b in zip(medians['baseline'],medians['bounded'])]
                result[phase]['paired_block_delta_pct']=summary(deltas)
                result[phase]['paired_block_deltas_pct']=deltas
            # Descriptive empirical envelope, not a confidence interval or equivalence test.
            result['noise_envelope_pct']=max(abs(d) for d in result['noise']['paired_block_deltas_pct'])
            report['results'].append(result)
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))

if __name__=='__main__': main()
