"""Caracteriza Tasks e rodadas sem truncamento executando a revisão Python congelada."""
import importlib.util
import hashlib
import json
import sys
import tempfile
import logging
import os
from dataclasses import asdict
from pathlib import Path

reference, destination = map(Path, sys.argv[1:])
sys.path.insert(0, str(reference / 'backend'))
from app import orq, orq_timeline, orq_start, pi_catalog, pi_models
revision = 'd2a42410f9fe6df4ea491b296c7fea7b98d8b844'
engine = orq_start._orq()
values = [1, 2**63, 2**64-1, 2**64, 2**127, 2**128+123, 10**90+7, -(2**64), -(2**128)-5]
cases=[]
for task in values:
    cfg={'arbiter':'arb','auto':True}
    events=[{'tipo':'execucao_inicio','gid':'g','ts':'2026-10-01T10:00:00+00:00'},
            {'tipo':'task_inicio','task':task,'executor':'ex','par':'rev','titulo':'Task sintética','ts':'2026-10-01T10:01:00+00:00'},
            {'tipo':'entrega','task':task,'rodada':abs(task),'ts':'2026-10-01T10:02:00+00:00'},
            {'tipo':'veredito','task':task,'rodada':abs(task),'resultado':'aprova','fase':'final','ts':'2026-10-01T10:03:00+00:00'}]
    closed=[{'task':task,'hash':'abcdef','ts':'2026-10-01T10:04:00+00:00'}]
    with tempfile.TemporaryDirectory() as temporary:
        directory=Path(temporary)/"run";directory.mkdir()
        (directory/'orq.json').write_text(json.dumps(cfg),encoding='utf-8')
        (directory/'eventos.jsonl').write_text(''.join(json.dumps(v)+'\n' for v in events),encoding='utf-8')
        (directory/'closed.jsonl').write_text(''.join(json.dumps(v)+'\n' for v in closed),encoding='utf-8')
        state=engine.state(directory)
        state['waiting']=sorted(state['waiting'])
        (directory/'closed.jsonl').write_text('',encoding='utf-8')
        state_open=engine.state(directory)
        state_open['waiting']=sorted(state_open['waiting'])
        cases.append({'task_digits':str(task),'config':cfg,'events':events,'closed':closed,'state':state,'state_open':state_open,
                      'detail':{**asdict(orq._monta('run',events)),'metadata':orq.run_metadata(directory)},
                      'summary':asdict(orq._monta('run',events)),'cards':orq.fichas([orq._monta('run',events)]),
                      'team':orq_timeline._team(engine,cfg,state_open,events),
                      'rows':orq_timeline._task_rows(events,[],set()),
                      'integrated_rows':orq_timeline._task_rows(events,[],{task})})
entries=[]
for task in values:
    for obj in [{'kind':'notice','task':task,'text':'mensagem sintética'},
                {'kind':'notice','text':f'Task {abs(task)} terminou'},
                {'kind':'woke','text':f'[T{abs(task)} r{abs(task)}] mensagem sintética'}]:
        entries.append({'input':obj,'entry':orq_timeline.entry(obj,None),'id':orq_timeline.event_id(obj)})
mixed_events=[event for case in reversed(cases) for event in case['events'] if event['tipo']!='execucao_inicio']
raw_cases=[]
for obj in [{'kind':'notice','extra':{'$serde_json::private::Number':'literal','n':10**90},'text':'chave literal'},
            {'kind':'notice','extra':{'$serde_json::private::RawValue':'literal'},'text':'chave literal'},
            {'kind':'notice','extra':{'\ud800':'um','\ue000':'dois','\U00010000':'três'},'text':'ordem de chaves'},
            {'kind':'notice','task':10**90,'text':'surrogate: \ud800'},
            {'kind':'notice','text':'números não finitos','extra':[float('nan'),float('inf'),-float('inf')]},
            {'kind':'notice','task':10**90+0.0,'text':'float largo'},
            {'kind':'notice','task':' +١_٢٣٤ ','text':'inteiro Unicode'},
            {'kind':'notice','task':'1__2','text':'inteiro inválido'}]:
    raw_cases.append({'raw':json.dumps(obj),'dump':json.dumps(obj,sort_keys=True),'entry_raw':json.dumps(orq_timeline.entry(obj,None)),'id':orq_timeline.event_id(obj)})
with tempfile.TemporaryDirectory() as temporary:
    directory=Path(temporary)
    path=directory/'jev-shadow.jsonl'
    shadow={'ts':'2026-10-01T10:00:00+00:00','text':'mensagem sintética','p':float('nan'),'probs':{'bom':0.75,'ruim':float('nan')},'mode':'active','choice':'wake'}
    path.write_text(json.dumps(shadow)+'\n',encoding='utf-8')
    obj={'kind':'woke','from':'fixture','text':'acordou o árbitro: mensagem sintética','ts':'2026-10-01T10:00:01+00:00'}
    shadow_case={'raw':json.dumps(shadow),'input':obj,'entry':orq_timeline.entry(obj,orq_timeline.RunFiles(directory))}
    log=[]
    class Capture(logging.Handler):
        def emit(self,record):log.append(record.getMessage())
    handler=Capture();orq_timeline._log.addHandler(handler)
    broken=directory/'blocked';broken.write_text('x',encoding='utf-8')
    files=orq_timeline.RunFiles(broken)
    files._jev_index();files._jev_index()
    warning_count=len(log)
    orq_timeline._log.removeHandler(handler)
plan_text='## Tasks\n\n| # | What it is | Files | Verification | Wave | Risk | Roteiro |\n|---|---|---|---|---|---|---|\n'+''.join(f'| {n} | Task sintética | `src/main.rs` | check | 1 | low | |\n' for n in values)
labels=[]
for value in ['1__000','_1000','1000_','١_٠٠٠','+１２３４',' \x1c１２３４\x1f',True,1000.9,2**128,None,0,-1001,float('nan'),float('inf'),-float('inf')]:
    raw=json.dumps({'models':[{'provider':'fixture','id':'one','contextWindow':value,'maxTokens':value}]})
    try: labels.append({'raw':raw,'output':pi_catalog.parse_omp(raw)})
    except Exception as error: labels.append({'raw':raw,'error':type(error).__name__})
with tempfile.TemporaryDirectory() as temporary:
    config=Path(temporary); path=pi_models.sidecar_path('fixture.jsonl',config);path.parent.mkdir(parents=True)
    stamp=1800000000000000000
    path.write_text(json.dumps({'models':[],'current':{'provider':'first'}}),encoding='utf-8');os.utime(path,ns=(stamp,stamp))
    first=pi_models.provider_atual('fixture.jsonl',config)
    path.write_text(json.dumps({'models':[],'current':{'provider':'second'}}),encoding='utf-8');os.utime(path,ns=(stamp+100,stamp+100))
    mtime={'stamp_ns':stamp,'delta_ns':100,'first':first,'after':pi_models.provider_atual('fixture.jsonl',config)}
result={'source_commit':revision,'command':'python crates/hangar-server/src/orchestration/fixtures/generate_bigint.py <referência Python congelada> <saída>',
        'source_hashes':{name:hashlib.sha256((reference/name).read_bytes()).hexdigest() for name in ['backend/app/orq.py','backend/app/orq_timeline.py','skills/orquestrar/scripts/orq.py']},
        'cases':cases,'entries':entries,'mixed':{'events':mixed_events,'rows':orq_timeline._task_rows(mixed_events,[],set()),'summary':asdict(orq._monta('run',mixed_events))},
        'plan':{'input':plan_text,'output':engine.plan_tasks(plan_text)},'labels':labels,'mtime':mtime,
        'raw_cases':raw_cases,'shadow':shadow_case,'metadata_warning_count':warning_count,
        'code':[{'input':text,'output':orq_timeline._code(text),'obj':{'kind':'woke','text':f'acordou o árbitro: {text}'},'entry':orq_timeline.entry({'kind':'woke','text':f'acordou o árbitro: {text}'},None)} for text in ['Veja src/main.rs','Veja fooBar','Veja src/main.rs.','Veja fooBar, agora','Veja `fooBar`','foo_bar','Veja src/main.rs\n','Veja função','Veja fooBar!']],
        'integers':[{'input':value,'digits':str(value) if orq._int_ou_none(value) is not None else None} for value in values+[10**400,1.0,True,None,'1']]}
destination.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(f'{len(cases)} execuções e {len(entries)} linhas caracterizadas na referência Python')
