"""Validate the curated full-context corpus without network access or API keys."""
import json
from pathlib import Path

FILES = ('profile', 'skills', 'experience', 'education', 'projects', 'testimonials', 'faq')

def validate(directory=Path('static/data')):
    data = {name: json.loads((directory / f'{name}.json').read_text(encoding='utf-8')) for name in FILES}
    size = len(json.dumps(data, ensure_ascii=False, separators=(',', ':')).encode())
    assert size <= 64000, f'Knowledge grew beyond the reviewed full-context limit: {size} bytes'
    assert data['profile']['links']['resume_url'] == 'https://cv.zqsdev.com/'
    assert not data['profile'].get('resume_variants'), 'One canonical CV, two languages'
    assert data['experience'][0]['company'].startswith('Studi')
    for item in data['experience']:
        assert item['title'] and item['company'] and item['highlights']
    for section in ('projects', 'publications', 'awards'):
        assert data['projects'][section], f'Empty enrichment section: {section}'
    assert data['testimonials'] and data['education']
    for item in data['faq']:
        assert item['question'] and item['answer']
    return data, size

if __name__ == '__main__':
    _, size = validate()
    print(f'Curated knowledge validated: {len(FILES)} sources, {size} UTF-8 bytes; no embeddings required.')
