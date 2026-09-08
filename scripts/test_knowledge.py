import json
import unittest
from pathlib import Path
from scripts.validate_knowledge import validate

ROOT = Path(__file__).resolve().parent.parent

class KnowledgeTests(unittest.TestCase):
    def test_current_facts_and_size(self):
        data, size = validate(ROOT / 'static/data')
        self.assertLess(size, 64000)
        studi = ' '.join(data['experience'][0]['highlights'])
        for fact in ('50,000', 'LangChain', 'Langfuse', 'CEO', '800'):
            self.assertIn(fact, studi)
        self.assertEqual(data['experience'][1]['end'], 'Mar 2026')
        self.assertEqual(data['experience'][2]['end'], 'Mar 2023')

    def test_historical_enrichment_preserved_exactly(self):
        data, _ = validate(ROOT / 'static/data')
        fixture = json.loads((ROOT / 'scripts/fixtures/legacy_enrichment.json').read_text(encoding='utf-8'))
        for key in ('education', 'testimonials'):
            self.assertEqual(data[key], fixture[key])
        for key in ('publications', 'awards'):
            self.assertEqual(data['projects'][key], fixture[key])
        for project in fixture['projects']:
            self.assertIn(project, data['projects']['projects'])
