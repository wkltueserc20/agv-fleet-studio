"""Read-only regression check against the running five-service simulator."""
import json
import time
import urllib.request


def get(port, path):
    with urllib.request.urlopen(f'http://127.0.0.1:{port}{path}', timeout=10) as response:
        return json.load(response)


central = get(8210, '/api/snapshot')
assert central['role'] == 'central'
assert len(central['vehicles']) == 3
assert central['vehicle']['vehicle_id'] == 'CT-SIM-01'
assert get(8210, '/api/vehicle')['vehicle_id'] == 'CT-SIM-01'
assert len(get(8210, '/api/vehicles')['vehicles']) == 3
# A real external state query remains supported; snapshot aggregation must not
# create an additional HTTP request from RCS back to itself.
assert get(8210, '/api/state')['role'] == 'rcs'
assert get(8211, '/api/snapshot')['rcs']['role'] == 'rcs'
for index, port in enumerate((8212, 8213, 8214), 1):
    snapshot = get(port, '/api/snapshot')
    assert snapshot['role'] == 'agv'
    assert snapshot['vehicle']['vehicle_id'] == f'CT-SIM-0{index}'
    assert snapshot['rcs'] is None and len(snapshot['vehicles']) == 1

# Trace writes finish just after the response is sent.
time.sleep(0.2)
assert not any(trace['source'] == 'rcs' and trace['target'] == 'rcs'
               for trace in get(8211, '/api/traces')['traces'])
for port in (8212, 8213, 8214):
    states = [trace for trace in get(port, '/api/traces')['traces']
              if trace['method'] == 'GET' and trace['path'] == '/api/state']
    assert states, f'No RCS state query on {port}'
    assert all(trace['source'] == 'rcs' for trace in states), states
    assert not any(trace['source'] == 'central'
                   for trace in get(port, '/api/traces')['traces'])

catalog = get(8211, '/api/catalog')['entries']
legacy = next(entry for entry in catalog
              if entry['role'] == 'central' and entry['path'] == '/api/vehicle')
assert legacy['direction'] == '介接服務 → RCS → AGV'
print('PASS: central uses RCS; vehicle pages read only local state; AGV state queries originate from RCS.')
